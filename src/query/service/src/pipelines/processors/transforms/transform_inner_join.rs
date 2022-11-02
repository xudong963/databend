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

use std::sync::Arc;

use common_datablocks::DataBlock;
use common_exception::Result;

use crate::pipelines::processors::transforms::Compactor;
use crate::pipelines::processors::HashJoinState;
use crate::pipelines::processors::TransformCompact;

pub struct InnerJoinCompactor {
    hash_join_state: Arc<dyn HashJoinState>,
}

impl InnerJoinCompactor {
    pub fn create(hash_join_state: Arc<dyn HashJoinState>) -> Self {
        InnerJoinCompactor { hash_join_state }
    }
}

impl Compactor for InnerJoinCompactor {
    fn name() -> &'static str {
        "InnerJoinCompactor"
    }

    fn interrupt(&self) {
        self.hash_join_state.interrupt();
    }

    // `compact_final` is called when all the blocks are pushed
    fn compact_final(&self, blocks: &[DataBlock]) -> Result<Vec<DataBlock>> {
        self.hash_join_state.inner_join_blocks(blocks)
    }
}

pub type TransformInnerJoin = TransformCompact<InnerJoinCompactor>;
