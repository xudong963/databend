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

use std::sync::Arc;

use databend_common_catalog::table_context::TableContext;
use databend_common_expression::DataSchema;
use databend_common_pipeline_core::Pipe;
use databend_common_sql::executor::physical_plans::MergeInto;
use databend_common_storages_fuse::operations::MergeIntoSplitProcessor;
use databend_common_storages_fuse::FuseTable;

use crate::pipelines::PipelineBuilder;

impl PipelineBuilder {
    pub(crate) fn build_merge_into_v2(&mut self, merge_into: &MergeInto) {
        let MergeInto {
            input,
            table_info,
            catalog_info,
            unmatched,
            matched,
            field_index_of_input_schema,
            row_id_idx,
            segments,
            execution_mode,
            merge_type,
            can_try_update_column_only,
            merge_into_split_idx,
            ..
        } = merge_into;

        self.build_pipeline(input)?;
        self.main_pipeline
            .try_resize(self.ctx.get_settings().get_max_threads()? as usize)?;

        let tbl = self
            .ctx
            .build_table_by_table_info(catalog_info, table_info, None)?;

        let table_schema = Arc::new(DataSchema::from(tbl.schema_with_stream()));
        // First, split the input data into three parts: update, delete, insert
        if let Some(split_idx) = merge_into_split_idx {
            let mut items = Vec::with_capacity(self.main_pipeline.output_len());
            let output_len = self.main_pipeline.output_len();
            for _ in 0..output_len {
                let merge_into_split_processor = MergeIntoSplitProcessor::create(
                    self.ctx.clone(),
                    *split_idx as u32,
                    merge_type.clone(),
                    matched,
                    field_index_of_input_schema,
                    row_id_idx,
                    table_schema.clone(),
                )?;
                items.push(merge_into_split_processor.into_pipe_item());
            }
            self.main_pipeline
                .add_pipe(Pipe::create(output_len, output_len * 3, items));
        }
    }
}
