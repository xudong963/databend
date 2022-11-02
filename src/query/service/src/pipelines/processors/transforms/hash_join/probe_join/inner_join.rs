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

use std::iter::repeat;
use std::iter::TrustedLen;
use std::sync::atomic::Ordering;

use common_catalog::table_context::TableContext;
use common_datablocks::DataBlock;
use common_exception::ErrorCode;
use common_exception::Result;
use common_hashtable::HashtableEntryRefLike;
use common_hashtable::HashtableLike;

use crate::pipelines::processors::transforms::hash_join::row::RowPtr;
use crate::pipelines::processors::transforms::hash_join::ProbeState;
use crate::pipelines::processors::JoinHashTable;

impl JoinHashTable {
    pub(crate) fn probe_inner_join<'a, H: HashtableLike<Value = Vec<RowPtr>>, IT>(
        &self,
        hash_table: &H,
        probe_state: &mut ProbeState,
        keys_iter: IT,
        input: &DataBlock,
    ) -> Result<Vec<DataBlock>>
    where
        IT: Iterator<Item = H::KeyRef<'a>> + TrustedLen,
        H::Key: 'a,
    {
        let valids = &probe_state.valids;
        let block_size = self
            .ctx
            .get_settings()
            .get_max_block_size()
            .unwrap_or(65535) as usize;

        // The inner join will return multiple data blocks of similar size
        let mut probed_blocks = vec![];
        let mut probe_indexes = Vec::with_capacity(block_size);
        let mut build_indexes = Vec::with_capacity(block_size);

        for (i, key) in keys_iter.enumerate() {
            // If the join is derived from correlated subquery, then null equality is safe.
            let probe_result_ptr = if self.hash_join_desc.from_correlated_subquery {
                hash_table.entry(key)
            } else {
                self.probe_key(hash_table, key, valids, i)
            };

            if let Some(v) = probe_result_ptr {
                let probed_rows = v.get();

                if probe_indexes.len() + probed_rows.len() < probe_indexes.capacity() {
                    build_indexes.extend_from_slice(probed_rows);
                    probe_indexes.extend(repeat(i as u32).take(probed_rows.len()));
                } else {
                    let mut index = 0_usize;
                    let mut remain = probed_rows.len();

                    while index < probed_rows.len() {
                        if probe_indexes.len() + remain < probe_indexes.capacity() {
                            build_indexes.extend_from_slice(&probed_rows[index..]);
                            probe_indexes.extend(std::iter::repeat(i as u32).take(remain));
                            index += remain;
                        } else {
                            if self.interrupt.load(Ordering::Relaxed) {
                                return Err(ErrorCode::AbortedQuery(
                                    "Aborted query, because the server is shutting down or the query was killed.",
                                ));
                            }

                            let addition = probe_indexes.capacity() - probe_indexes.len();
                            let new_index = index + addition;

                            build_indexes.extend_from_slice(&probed_rows[index..new_index]);
                            probe_indexes.extend(repeat(i as u32).take(addition));

                            probed_blocks.push(self.merge_eq_block(
                                &self.row_space.gather(&build_indexes)?,
                                &DataBlock::block_take_by_indices(input, &probe_indexes)?,
                            )?);

                            index = new_index;
                            remain -= addition;

                            build_indexes.clear();
                            probe_indexes.clear();
                        }
                    }
                }
            }
        }

        let probe_block = DataBlock::block_take_by_indices(input, &probe_indexes)?;

        if self.hash_join_desc.other_predicate.is_none() {
            let mut rest_build_indexes = self
                .hash_join_desc
                .right_join_desc
                .rest_build_indexes
                .write();
            rest_build_indexes.extend(build_indexes);
            let mut rest_probe_blocks = self
                .hash_join_desc
                .right_join_desc
                .rest_probe_blocks
                .write();
            rest_probe_blocks.push(probe_block);
            return Ok(probed_blocks);
        }

        probed_blocks
            .push(self.merge_eq_block(&self.row_space.gather(&build_indexes)?, &probe_block)?);

        let func_ctx = self.ctx.try_get_function_context()?;
        let mut filtered_blocks = Vec::with_capacity(probed_blocks.len());
        let other_predicate = self.hash_join_desc.other_predicate.as_ref().unwrap();
        for probed_block in probed_blocks {
            if self.interrupt.load(Ordering::Relaxed) {
                return Err(ErrorCode::AbortedQuery(
                    "Aborted query, because the server is shutting down or the query was killed.",
                ));
            }

            let predicate = other_predicate.eval(&func_ctx, &probed_block)?;
            let res = DataBlock::filter_block(probed_block, predicate.vector())?;

            if !res.is_empty() {
                filtered_blocks.push(res);
            }
        }

        Ok(filtered_blocks)
    }
}
