// Copyright 2022 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;

use async_recursion::async_recursion;
use common_ast::ast::Cte;
use common_ast::ast::Expr;
use common_ast::ast::Join;
use common_ast::ast::JoinCondition;
use common_ast::ast::JoinOperator;
use common_ast::ast::OrderByExpr;
use common_ast::ast::Query;
use common_ast::ast::SelectStmt;
use common_ast::ast::SelectTarget;
use common_ast::ast::SetExpr;
use common_ast::ast::SetOperator;
use common_ast::ast::TableReference;
use common_datavalues::type_coercion::merge_types;
use common_exception::ErrorCode;
use common_exception::Result;

use crate::sql::binder::scalar_common::split_conjunctions;
use crate::sql::optimizer::SExpr;
use crate::sql::planner::binder::scalar::ScalarBinder;
use crate::sql::planner::binder::BindContext;
use crate::sql::planner::binder::Binder;
use crate::sql::plans::BoundColumnRef;
use crate::sql::plans::Filter;
use crate::sql::plans::JoinType;
use crate::sql::plans::Scalar;

// A normalized IR for `SELECT` clause.
#[derive(Debug, Default)]
pub struct SelectList<'a> {
    pub items: Vec<SelectItem<'a>>,
}

#[derive(Debug)]
pub struct SelectItem<'a> {
    pub select_target: &'a SelectTarget<'a>,
    pub scalar: Scalar,
    pub alias: String,
}

impl<'a> Binder<'_> {
    pub(super) async fn bind_select_stmt(
        &mut self,
        bind_context: &BindContext,
        stmt: &SelectStmt<'a>,
        order_by: &[OrderByExpr<'a>],
    ) -> Result<(SExpr, BindContext)> {
        let (mut s_expr, mut from_context) = if stmt.from.is_empty() {
            self.bind_one_table(bind_context, stmt).await?
        } else {
            let cross_joins = stmt
                .from
                .iter()
                .cloned()
                .reduce(|left, right| TableReference::Join {
                    span: &[],
                    join: Join {
                        op: JoinOperator::CrossJoin,
                        condition: JoinCondition::None,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                })
                .unwrap();
            self.bind_table_reference(bind_context, &cross_joins)
                .await?
        };

        if let Some(expr) = &stmt.selection {
            s_expr = self.bind_where(&from_context, expr, s_expr).await?;
        }

        // Generate a analyzed select list with from context
        let mut select_list = self
            .normalize_select_list(&from_context, &stmt.select_list)
            .await?;

        let (mut scalar_items, projections) = self.analyze_projection(&select_list)?;

        // This will potentially add some alias group items to `from_context` if find some.
        self.analyze_group_items(&mut from_context, &select_list, &stmt.group_by)
            .await?;

        self.analyze_aggregate_select(&mut from_context, &mut select_list)?;

        let having = if let Some(having) = &stmt.having {
            Some(
                self.analyze_aggregate_having(&mut from_context, &select_list, having)
                    .await?,
            )
        } else {
            None
        };

        let order_items = self
            .analyze_order_items(
                &from_context,
                &mut scalar_items,
                &projections,
                order_by,
                stmt.distinct,
            )
            .await?;

        if !from_context.aggregate_info.aggregate_functions.is_empty() || !stmt.group_by.is_empty()
        {
            s_expr = self.bind_aggregate(&mut from_context, s_expr).await?;
        }

        if let Some((having, span)) = having {
            s_expr = self
                .bind_having(&from_context, having, span, s_expr)
                .await?;
        }

        if stmt.distinct {
            s_expr = self.bind_distinct(&from_context, &projections, &mut scalar_items, s_expr)?;
        }

        if !order_by.is_empty() {
            s_expr = self
                .bind_order_by(
                    &from_context,
                    order_items,
                    &select_list,
                    &mut scalar_items,
                    s_expr,
                )
                .await?;
        }

        s_expr = self.bind_projection(&mut from_context, &projections, &scalar_items, s_expr)?;

        let mut output_context = BindContext::new();
        output_context.parent = from_context.parent;
        output_context.columns = from_context.columns;

        Ok((s_expr, output_context))
    }

    #[async_recursion]
    pub(crate) async fn bind_set_expr(
        &mut self,
        bind_context: &BindContext,
        set_expr: &SetExpr<'a>,
        order_by: &[OrderByExpr<'a>],
    ) -> Result<(SExpr, BindContext)> {
        match set_expr {
            SetExpr::Select(stmt) => self.bind_select_stmt(bind_context, stmt, order_by).await,
            SetExpr::Query(stmt) => self.bind_query(bind_context, stmt).await,
            SetExpr::SetOperation(set_operation) => {
                self.bind_set_operator(
                    bind_context,
                    &set_operation.left,
                    &set_operation.right,
                    &set_operation.op,
                    &set_operation.all,
                )
                .await
            }
        }
    }

    pub(crate) async fn bind_query(
        &mut self,
        bind_context: &BindContext,
        query: &Query<'a>,
    ) -> Result<(SExpr, BindContext)> {
        if let Some(with) = &query.with {
            for cte in with.ctes.iter() {
                let table_name = cte.alias.name.name.clone();
                if self.ctes_map.contains_key(&table_name) {
                    return Err(ErrorCode::SemanticError(format!(
                        "duplicate cte {table_name}"
                    )));
                }
                self.ctes_map.insert(table_name, cte.clone());
            }
        }
        let (mut s_expr, mut bind_context) = match query.body {
            SetExpr::Select(_) | SetExpr::Query(_) => {
                self.bind_set_expr(bind_context, &query.body, &query.order_by)
                    .await?
            }
            SetExpr::SetOperation(_) => {
                let (mut s_expr, bind_context) =
                    self.bind_set_expr(bind_context, &query.body, &[]).await?;
                if !query.order_by.is_empty() {
                    s_expr = self
                        .bind_order_by_for_set_operation(&bind_context, s_expr, &query.order_by)
                        .await?;
                }
                (s_expr, bind_context)
            }
        };

        if !query.limit.is_empty() {
            if query.limit.len() == 1 {
                s_expr = self
                    .bind_limit(&bind_context, s_expr, Some(&query.limit[0]), &query.offset)
                    .await?;
            } else {
                s_expr = self
                    .bind_limit(
                        &bind_context,
                        s_expr,
                        Some(&query.limit[1]),
                        &Some(query.limit[0].clone()),
                    )
                    .await?;
            }
        } else if query.offset.is_some() {
            s_expr = self
                .bind_limit(&bind_context, s_expr, None, &query.offset)
                .await?;
        }

        if let Some(format) = &query.format {
            bind_context.resolve_format(format.clone())?
        }

        Ok((s_expr, bind_context))
    }

    pub(super) async fn bind_where(
        &mut self,
        bind_context: &BindContext,
        expr: &Expr<'a>,
        child: SExpr,
    ) -> Result<SExpr> {
        let mut scalar_binder = ScalarBinder::new(
            bind_context,
            self.ctx.clone(),
            &self.name_resolution_ctx,
            self.metadata.clone(),
            &[],
        );
        let (scalar, _) = scalar_binder.bind(expr).await?;
        let filter_plan = Filter {
            predicates: split_conjunctions(&scalar),
            is_having: false,
        };
        let new_expr = SExpr::create_unary(filter_plan.into(), child);
        Ok(new_expr)
    }

    pub(super) async fn bind_set_operator(
        &mut self,
        bind_context: &BindContext,
        left: &SetExpr<'a>,
        right: &SetExpr<'a>,
        op: &SetOperator,
        all: &bool,
    ) -> Result<(SExpr, BindContext)> {
        let (left_expr, mut left_bind_context) =
            self.bind_set_expr(bind_context, left, &[]).await?;
        let (right_expr, mut right_bind_context) =
            self.bind_set_expr(bind_context, right, &[]).await?;
        if left_bind_context.columns.len() != right_bind_context.columns.len() {
            return Err(ErrorCode::SemanticError(
                "SetOperation must have the same number of columns",
            ));
        } else {
            for (left_col, right_col) in left_bind_context
                .columns
                .iter_mut()
                .zip(right_bind_context.columns.iter_mut())
            {
                if left_col.data_type != right_col.data_type {
                    let coercion_type = merge_types(&left_col.data_type, &right_col.data_type)?;
                    left_col.data_type = Box::new(coercion_type.clone());
                    right_col.data_type = Box::new(coercion_type);
                }
                if left_col.data_type != right_col.data_type {
                    return Err(ErrorCode::SemanticError(
                        "SetOperation's types cannot be matched",
                    ));
                }
            }
        }
        match (op, all) {
            (SetOperator::Intersect, false) => {
                // Transfer Intersect to Semi join
                self.bind_intersect(left_bind_context, right_bind_context, left_expr, right_expr)
            }
            (SetOperator::Except, false) => {
                // Transfer Except to Anti join
                self.bind_except(left_bind_context, right_bind_context, left_expr, right_expr)
            }
            _ => Err(ErrorCode::UnImplement(
                "Unsupported query type, currently, databend only support intersect distinct and except distinct",
            )),
        }
    }

    fn bind_intersect(
        &mut self,
        left_context: BindContext,
        right_context: BindContext,
        left_expr: SExpr,
        right_expr: SExpr,
    ) -> Result<(SExpr, BindContext)> {
        self.bind_intersect_or_except(
            left_context,
            right_context,
            left_expr,
            right_expr,
            JoinType::Semi,
        )
    }

    fn bind_except(
        &mut self,
        left_context: BindContext,
        right_context: BindContext,
        left_expr: SExpr,
        right_expr: SExpr,
    ) -> Result<(SExpr, BindContext)> {
        self.bind_intersect_or_except(
            left_context,
            right_context,
            left_expr,
            right_expr,
            JoinType::Anti,
        )
    }

    fn bind_intersect_or_except(
        &mut self,
        left_context: BindContext,
        right_context: BindContext,
        left_expr: SExpr,
        right_expr: SExpr,
        join_type: JoinType,
    ) -> Result<(SExpr, BindContext)> {
        let left_expr = self.bind_distinct(
            &left_context,
            left_context.all_column_bindings(),
            &mut HashMap::new(),
            left_expr,
        )?;
        let mut left_conditions = Vec::with_capacity(left_context.columns.len());
        let mut right_conditions = Vec::with_capacity(right_context.columns.len());
        assert_eq!(left_context.columns.len(), right_context.columns.len());
        for (left_column, right_column) in left_context
            .columns
            .iter()
            .zip(right_context.columns.iter())
        {
            left_conditions.push(
                BoundColumnRef {
                    column: left_column.clone(),
                }
                .into(),
            );
            right_conditions.push(
                BoundColumnRef {
                    column: right_column.clone(),
                }
                .into(),
            );
        }
        let s_expr = self.bind_join_with_type(
            join_type,
            left_conditions,
            right_conditions,
            vec![],
            left_expr,
            right_expr,
        )?;
        Ok((s_expr, left_context))
    }
}
