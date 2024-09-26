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
use databend_common_expression::BlockEntry;
use databend_common_expression::DataBlock;
use databend_common_expression::Value;

use crate::pipelines::processors::transforms::hash_join::HashJoinProbeState;
use crate::pipelines::processors::transforms::hash_join::ProbeState;

impl HashJoinProbeState {
    pub(crate) fn cross_join(
        &self,
        input: DataBlock,
        _probe_state: &mut ProbeState,
    ) -> Result<Vec<DataBlock>> {
        let build_state = unsafe { &*self.hash_join_state.build_state.get() };
        let build_blocks = &build_state.generation_state.chunks;
        let build_num_rows = build_blocks
            .iter()
            .fold(0, |acc, block| acc + block.num_rows());
        let input_num_rows = input.num_rows();
        if build_num_rows == 0 || input_num_rows == 0 {
            return Ok(vec![]);
        }
        let mut probe_block = input.project(&self.probe_projections);
        let build_block = DataBlock::concat(build_blocks)?;
        if build_num_rows == 1 {
            for col in build_block.columns() {
                let value_ref = col.value.as_ref();
                let scalar = unsafe { value_ref.index_unchecked(0) };
                probe_block.add_column(BlockEntry::new(
                    col.data_type.clone(),
                    Value::Scalar(scalar.to_owned()),
                ));
            }
            return Ok(vec![probe_block]);
        }
        let mut result_blocks = Vec::with_capacity(input_num_rows);
        for i in 0..build_num_rows {
            result_blocks.push(self.merge_with_constant_block(
                &build_block,
                &probe_block,
                i,
            )?);
        }
        Ok(result_blocks)
    }

    // Merge build block(1 row) and probe block
    pub(crate) fn merge_with_constant_block(
        &self,
        build_block: &DataBlock,
        probe_block: &DataBlock,
        take_index: usize,
    ) -> Result<DataBlock> {
        let mut probe_block = probe_block.clone();
        for col in build_block.columns() {
            let value_ref = col.value.as_ref();
            let scalar = unsafe { value_ref.index_unchecked(take_index) };
            probe_block.add_column(BlockEntry::new(
                col.data_type.clone(),
                Value::Scalar(scalar.to_owned()),
            ));
        }
        Ok(probe_block)
    }
}
