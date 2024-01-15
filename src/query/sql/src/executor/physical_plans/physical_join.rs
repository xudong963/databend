// Copyright 2021 Datafuse Labs
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

use databend_common_exception::Result;

use crate::binder::JoinPredicate;
use crate::executor::explain::PlanStatsInfo;
use crate::executor::PhysicalPlan;
use crate::executor::PhysicalPlanBuilder;
use crate::optimizer::RelExpr;
use crate::optimizer::RelationalProperty;
use crate::optimizer::SExpr;
use crate::plans::Join;
use crate::plans::JoinType;
use crate::ColumnSet;
use crate::ScalarExpr;

pub enum PhysicalJoinType {
    Hash,
    // The first arg is range conditions, the second arg is other conditions
    RangeJoin(Vec<ScalarExpr>, Vec<ScalarExpr>),
}

// Choose physical join type by join conditions
pub fn physical_join(join: &Join, s_expr: &SExpr) -> Result<PhysicalJoinType> {
    if !join.left_conditions.is_empty() {
        // Contain equi condition, use hash join
        return Ok(PhysicalJoinType::Hash);
    }

    let left_prop = RelExpr::with_s_expr(s_expr.child(0)?).derive_relational_prop()?;
    let right_prop = RelExpr::with_s_expr(s_expr.child(1)?).derive_relational_prop()?;
    let mut range_conditions = vec![];
    let mut other_conditions = vec![];
    for condition in join.non_equi_conditions.iter() {
        check_condition(
            condition,
            &left_prop,
            &right_prop,
            &mut range_conditions,
            &mut other_conditions,
        )
    }

    if !range_conditions.is_empty() && matches!(join.join_type, JoinType::Inner | JoinType::Cross) {
        return Ok(PhysicalJoinType::RangeJoin(
            range_conditions,
            other_conditions,
        ));
    }

    // Leverage hash join to execute nested loop join
    Ok(PhysicalJoinType::Hash)
}

fn check_condition(
    expr: &ScalarExpr,
    left_prop: &RelationalProperty,
    right_prop: &RelationalProperty,
    range_conditions: &mut Vec<ScalarExpr>,
    other_conditions: &mut Vec<ScalarExpr>,
) {
    if let ScalarExpr::FunctionCall(func) = expr {
        if func.arguments.len() != 2
            || !matches!(func.func_name.as_str(), "gt" | "lt" | "gte" | "lte")
        {
            other_conditions.push(expr.clone());
            return;
        }
        let mut left = false;
        let mut right = false;
        for arg in func.arguments.iter() {
            let join_predicate = JoinPredicate::new(arg, left_prop, right_prop);
            match join_predicate {
                JoinPredicate::Left(_) => left = true,
                JoinPredicate::Right(_) => right = true,
                JoinPredicate::Both { .. } | JoinPredicate::Other(_) | JoinPredicate::ALL(_) => {
                    return;
                }
            }
        }
        if left && right {
            range_conditions.push(expr.clone());
            return;
        }
    }
    other_conditions.push(expr.clone());
}

impl PhysicalPlanBuilder {
    pub(crate) async fn build_join(
        &mut self,
        s_expr: &SExpr,
        join: &crate::plans::Join,
        required: ColumnSet,
        stat_info: PlanStatsInfo,
    ) -> Result<PhysicalPlan> {
        // 1. Prune unused Columns.
        let column_projections = required.clone().into_iter().collect::<Vec<_>>();
        let others_required = join
            .non_equi_conditions
            .iter()
            .fold(required.clone(), |acc, v| {
                acc.union(&v.used_columns()).cloned().collect()
            });
        let pre_column_projections = others_required.clone().into_iter().collect::<Vec<_>>();
        // Include columns referenced in left conditions and right conditions.
        let left_required = join
            .left_conditions
            .iter()
            .fold(required.clone(), |acc, v| {
                acc.union(&v.used_columns()).cloned().collect()
            })
            .union(&others_required)
            .cloned()
            .collect();
        let right_required = join
            .right_conditions
            .iter()
            .fold(required, |acc, v| {
                acc.union(&v.used_columns()).cloned().collect()
            })
            .union(&others_required)
            .cloned()
            .collect();

        // 2. Build physical plan.
        // Choose physical join type by join conditions
        let physical_join = physical_join(join, s_expr)?;
        match physical_join {
            PhysicalJoinType::Hash => {
                self.build_hash_join(
                    join,
                    s_expr,
                    (left_required, right_required),
                    pre_column_projections,
                    column_projections,
                    stat_info,
                )
                .await
            }
            PhysicalJoinType::RangeJoin(range, other) => {
                self.build_range_join(s_expr, left_required, right_required, range, other)
                    .await
            }
        }
    }
}
