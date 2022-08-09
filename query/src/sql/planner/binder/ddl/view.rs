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

use common_ast::ast::AlterViewStmt;
use common_ast::ast::CreateViewStmt;
use common_ast::ast::DropViewStmt;
use common_exception::Result;
use common_planners::AlterViewPlan;
use common_planners::CreateViewPlan;
use common_planners::DropViewPlan;

use crate::sessions::TableContext;
use crate::sql::binder::Binder;
use crate::sql::planner::semantic::normalize_identifier;
use crate::sql::plans::Plan;

impl<'a> Binder<'_> {
    pub(in crate::sql::planner::binder) async fn bind_create_view(
        &mut self,
        stmt: &CreateViewStmt<'a>,
    ) -> Result<Plan> {
        let CreateViewStmt {
            if_not_exists,
            catalog,
            database,
            view,
            query,
        } = stmt;

        let tenant = self.ctx.get_tenant();
        let catalog = catalog
            .as_ref()
            .map(|ident| normalize_identifier(ident, &self.name_resolution_ctx).name)
            .unwrap_or_else(|| self.ctx.get_current_catalog());
        let database = database
            .as_ref()
            .map(|ident| normalize_identifier(ident, &self.name_resolution_ctx).name)
            .unwrap_or_else(|| self.ctx.get_current_database());
        let viewname = normalize_identifier(view, &self.name_resolution_ctx).name;
        let subquery = format!("{}", query);

        let plan = CreateViewPlan {
            if_not_exists: *if_not_exists,
            tenant,
            catalog,
            database,
            viewname,
            subquery,
        };
        Ok(Plan::CreateView(Box::new(plan)))
    }

    pub(in crate::sql::planner::binder) async fn bind_alter_view(
        &mut self,
        stmt: &AlterViewStmt<'a>,
    ) -> Result<Plan> {
        let AlterViewStmt {
            catalog,
            database,
            view,
            query,
        } = stmt;

        let tenant = self.ctx.get_tenant();
        let catalog = catalog
            .as_ref()
            .map(|ident| normalize_identifier(ident, &self.name_resolution_ctx).name)
            .unwrap_or_else(|| self.ctx.get_current_catalog());
        let database = database
            .as_ref()
            .map(|ident| normalize_identifier(ident, &self.name_resolution_ctx).name)
            .unwrap_or_else(|| self.ctx.get_current_database());
        let viewname = normalize_identifier(view, &self.name_resolution_ctx).name;
        let subquery = format!("{}", query);

        let plan = AlterViewPlan {
            tenant,
            catalog,
            database,
            viewname,
            subquery,
        };
        Ok(Plan::AlterView(Box::new(plan)))
    }

    pub(in crate::sql::planner::binder) async fn bind_drop_view(
        &mut self,
        stmt: &DropViewStmt<'a>,
    ) -> Result<Plan> {
        let DropViewStmt {
            if_exists,
            catalog,
            database,
            view,
        } = stmt;

        let tenant = self.ctx.get_tenant();
        let catalog = catalog
            .as_ref()
            .map(|ident| normalize_identifier(ident, &self.name_resolution_ctx).name)
            .unwrap_or_else(|| self.ctx.get_current_catalog());
        let database = database
            .as_ref()
            .map(|ident| normalize_identifier(ident, &self.name_resolution_ctx).name)
            .unwrap_or_else(|| self.ctx.get_current_database());
        let viewname = normalize_identifier(view, &self.name_resolution_ctx).name;

        let plan = DropViewPlan {
            if_exists: *if_exists,
            tenant,
            catalog,
            database,
            viewname,
        };
        Ok(Plan::DropView(Box::new(plan)))
    }
}
