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

use std::collections::HashMap;

use databend_common_exception::Result;
use databend_common_expression::DataSchemaRef;
use databend_common_expression::FieldIndex;
use databend_common_expression::RemoteExpr;
use databend_common_meta_app::schema::CatalogInfo;
use databend_common_meta_app::schema::TableInfo;
use databend_storages_common_table_meta::meta::Location;

use crate::binder::MergeIntoType;
use crate::executor::physical_plan::PhysicalPlan;
use crate::plans::ExecutionMode;

pub type MatchExpr = Vec<(Option<RemoteExpr>, Option<Vec<(FieldIndex, RemoteExpr)>>)>;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MergeInto {
    pub plan_id: u32,
    pub input: Box<PhysicalPlan>,
    pub table_info: TableInfo,
    pub catalog_info: CatalogInfo,
    // (DataSchemaRef, Option<RemoteExpr>, Vec<RemoteExpr>,Vec<usize>) => (source_schema, condition, value_exprs)
    pub unmatched: Vec<(DataSchemaRef, Option<RemoteExpr>, Vec<RemoteExpr>)>,
    // the first option stands for the condition
    // the second option stands for update/delete
    pub matched: MatchExpr,
    // used to record the index of target table's field in merge_source_schema
    pub field_index_of_input_schema: HashMap<FieldIndex, usize>,
    pub row_id_idx: usize,
    pub segments: Vec<(usize, Location)>,
    pub execution_mode: ExecutionMode,
    pub merge_type: MergeIntoType,
    pub target_build_optimization: bool,
    pub can_try_update_column_only: bool,
    pub merge_into_split_idx: Option<usize>,
}

impl MergeInto {
    pub fn output_schema(&self) -> Result<DataSchemaRef> {
        todo!()
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MergeIntoAppendNotMatched {
    pub plan_id: u32,
    pub input: Box<PhysicalPlan>,
    pub table_info: TableInfo,
    pub catalog_info: CatalogInfo,
    // (DataSchemaRef, Option<RemoteExpr>, Vec<RemoteExpr>,Vec<usize>) => (source_schema, condition, value_exprs)
    pub unmatched: Vec<(DataSchemaRef, Option<RemoteExpr>, Vec<RemoteExpr>)>,
    pub input_schema: DataSchemaRef,
    pub merge_type: MergeIntoType,
    pub segments: Vec<(usize, Location)>,
}
