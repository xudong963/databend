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

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use databend_common_catalog::table_context::TableContext;
use databend_common_exception::Result;
use databend_common_expression::type_check::check;
use databend_common_expression::types::BooleanType;
use databend_common_expression::BlockMetaInfoDowncast;
use databend_common_expression::DataBlock;
use databend_common_expression::DataSchemaRef;
use databend_common_expression::FieldIndex;
use databend_common_expression::RawExpr;
use databend_common_expression::Value;
use databend_common_functions::BUILTIN_FUNCTIONS;
use databend_common_metrics::storage::*;
use databend_common_pipeline_core::processors::Event;
use databend_common_pipeline_core::processors::InputPort;
use databend_common_pipeline_core::processors::OutputPort;
use databend_common_pipeline_core::processors::Processor;
use databend_common_pipeline_core::processors::ProcessorPtr;
use databend_common_pipeline_core::PipeItem;
use databend_common_sql::binder::MergeIntoType;
use databend_common_sql::evaluator::BlockOperator;
use databend_common_sql::executor::physical_plans::MatchExpr;
use databend_common_storage::MergeStatus;

use crate::operations::merge_into::mutator::DeleteByExprMutator;
use crate::operations::merge_into::mutator::MergeIntoSplitMutator;
use crate::operations::merge_into::mutator::UpdateByExprMutator;
use crate::operations::BlockMetaIndex;

struct UpdateDataBlockMutation {
    update_mutator: UpdateByExprMutator,
}

struct DeleteDataBlockMutation {
    delete_mutator: DeleteByExprMutator,
}

enum MutationOperation {
    Update(UpdateDataBlockMutation),
    Delete(DeleteDataBlockMutation),
}

// There are two kinds of usage for this processor:
// 1. we will receive a probed datablock from join, and split it by rowid into matched block and unmatched block
// 2. we will receive a unmatched datablock, but this is an optimization for target table as build side. The unmatched
// datablock is a physical block's partial unmodified block. And its meta is a prefix(segment_id_block_id).
// we use the meta to distinct 1 and 2.
pub struct MergeIntoSplitProcessor {
    ctx: Arc<dyn TableContext>,
    input_port: Arc<InputPort>,
    output_port_update: Arc<OutputPort>,
    output_port_delete: Arc<OutputPort>,
    output_port_insert: Arc<OutputPort>,

    input_data: Option<DataBlock>,
    output_data_update: Option<DataBlock>,
    output_data_delete: Option<DataBlock>,
    output_data_insert: Option<DataBlock>,

    merge_into_split_mutator: MergeIntoSplitMutator,
    merge_into_type: MergeIntoType,
    mutation_ops: Vec<MutationOperation>,
    update_projections: Vec<usize>,
    target_table_schema: DataSchemaRef,
}

impl MergeIntoSplitProcessor {
    pub fn create(
        ctx: Arc<dyn TableContext>,
        split_idx: u32,
        merge_into_type: MergeIntoType,
        match_expr: &MatchExpr,
        field_index_of_input_schema: &HashMap<FieldIndex, usize>,
        row_id_idx: &usize,
        target_table_schema: DataSchemaRef,
    ) -> Result<Self> {
        let merge_into_split_mutator = MergeIntoSplitMutator::try_create(split_idx);
        let input_port = InputPort::create();
        let output_port_update = OutputPort::create();
        let output_port_delete = OutputPort::create();
        let output_port_insert = OutputPort::create();
        let mutation_ops =
            Self::get_mutation_ops(&ctx, match_expr, field_index_of_input_schema, row_id_idx)?;
        // `field_index_of_input_schema` contains all columns of target_table
        let mut update_projections = Vec::with_capacity(field_index_of_input_schema.len());
        for field_index in 0..field_index_of_input_schema.len() {
            update_projections.push(*field_index_of_input_schema.get(&field_index).unwrap());
        }
        Ok(Self {
            ctx,
            input_port,
            output_port_update,
            output_port_delete,
            output_port_insert,
            input_data: None,
            output_data_update: None,
            output_data_delete: None,
            output_data_insert: None,
            merge_into_split_mutator,
            merge_into_type,
            mutation_ops,
            update_projections,
            target_table_schema,
        })
    }

    pub fn into_pipe_item(self) -> PipeItem {
        let input = self.input_port.clone();
        let processor_ptr = ProcessorPtr::create(Box::new(self));
        PipeItem::create(processor_ptr, vec![input], vec![
            self.output_port_update.clone(),
            self.output_port_delete.clone(),
            self.output_port_insert.clone(),
        ])
    }

    fn get_mutation_ops(
        &self,
        ctx: &Arc<dyn TableContext>,
        match_expr: &MatchExpr,
        field_index_of_input_schema: &HashMap<FieldIndex, usize>,
        row_id_idx: &usize,
    ) -> Result<Vec<MutationOperation>> {
        let mut ops = Vec::new();
        for item in match_expr.iter() {
            // delete
            if item.1.is_none() {
                let filter = item.0.as_ref().map(|expr| expr.as_expr(&BUILTIN_FUNCTIONS));
                ops.push(MutationOperation::Delete(DeleteDataBlockMutation {
                    delete_mutator: DeleteByExprMutator::create(
                        filter.clone(),
                        ctx.get_function_context()?,
                        *row_id_idx,
                    ),
                }))
            } else {
                let update_lists = item.1.as_ref().unwrap();
                let filter = item
                    .0
                    .as_ref()
                    .map(|condition| condition.as_expr(&BUILTIN_FUNCTIONS));

                ops.push(MutationOperation::Update(UpdateDataBlockMutation {
                    update_mutator: UpdateByExprMutator::create(
                        filter,
                        ctx.get_function_context()?,
                        field_index_of_input_schema.clone(),
                        update_lists.clone(),
                    ),
                }))
            }
        }
        Ok(ops)
    }

    // The method splits matched data to update and delete data.
    fn split_matched_data(&mut self, data_block: DataBlock) -> Result<()> {
        let mut current_block = data_block;
        for (idx, op) in self.mutation_ops.iter().enumerate() {
            match op {
                MutationOperation::Update(update_mutation) => {
                    let stage_block = update_mutation
                        .update_mutator
                        .update_by_expr(current_block, idx == 0)?;
                    current_block = stage_block;
                }

                MutationOperation::Delete(delete_mutation) => {
                    let (stage_block, mut row_ids) = delete_mutation
                        .delete_mutator
                        .delete_by_expr(current_block, idx == 0)?;

                    // delete all
                    if !row_ids.is_empty() {
                        row_ids = row_ids.add_meta(Some(Box::new(RowIdKind::Delete)))?;
                        self.output_data_delete.push(row_ids);
                    }

                    if stage_block.is_empty() {
                        return Ok(());
                    }
                    current_block = stage_block;
                }
            }
        }

        let filter: Value<BooleanType> = current_block
            .get_by_offset(current_block.num_columns() - 1)
            .value
            .try_downcast()
            .unwrap();
        current_block = current_block.filter_boolean_value(&filter)?;

        if !current_block.is_empty() {
            self.ctx.add_merge_status(MergeStatus {
                insert_rows: 0,
                update_rows: current_block.num_rows(),
                deleted_rows: 0,
            });

            let op = BlockOperator::Project {
                projection: self.update_projections.clone(),
            };
            current_block = op.execute(&self.ctx.get_function_context()?, current_block)?;

            current_block = self.cast_data_type_for_merge(current_block)?;

            self.output_data_update = Some(current_block);
        }
        Ok(())
    }

    fn cast_data_type_for_merge(&self, current_block: DataBlock) -> Result<DataBlock> {
        // corner case: for merge into update, if the target table's column is not null,
        // for example, target table has three columns like (a,b,c), and we use update set target_table.a = xxx,
        // it's fine because we have cast the xxx'data_type into a's data_type in `generate_update_list()`,
        // but for b,c, the hash table will transform the origin data_type (b_type,c_type) into
        // (nullable(b_type),nullable(c_type)), so we will get datatype not match error, let's transform
        // them back here.
        let current_columns = current_block.columns();
        assert_eq!(
            self.target_table_schema.fields.len(),
            current_columns.len(),
            "target table columns and current columns length mismatch"
        );
        let cast_exprs = current_columns
            .iter()
            .enumerate()
            .map(|(idx, col)| {
                check(
                    &RawExpr::Cast {
                        span: None,
                        is_try: false,
                        expr: Box::new(RawExpr::ColumnRef {
                            span: None,
                            id: idx,
                            data_type: col.data_type.clone(),
                            display_name: "".to_string(),
                        }),
                        dest_type: self.target_table_schema.fields[idx].data_type().clone(),
                    },
                    &BUILTIN_FUNCTIONS,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let cast_operator = BlockOperator::Map {
            exprs: cast_exprs,
            projections: Some((current_columns.len()..current_columns.len() * 2).collect()),
        };
        cast_operator.execute(&self.ctx.get_function_context()?, current_block)
    }
}

impl Processor for MergeIntoSplitProcessor {
    fn name(&self) -> String {
        "MergeIntoSplit".to_owned()
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }

    fn event(&mut self) -> Result<Event> {
        let finished = self.input_port.is_finished()
            && self.output_data_update.is_none()
            && self.output_data_delete.is_none()
            && self.output_data_insert.is_none();
        if finished {
            self.output_port_update.finish();
            self.output_port_delete.finish();
            self.output_port_insert.finish();
            return Ok(Event::Finished);
        }

        let mut pushed_something = false;

        if self.output_port_update.can_push() {
            if let Some(update_data) = self.output_data_update.take() {
                self.output_port_update.push_data(Ok(update_data));
                pushed_something = true
            }
        }

        if self.output_port_delete.can_push() {
            if let Some(delete_data) = self.output_data_delete.take() {
                self.output_port_delete.push_data(Ok(delete_data));
                pushed_something = true
            }
        }

        if self.output_port_insert.can_push() {
            if let Some(insert_data) = self.output_port_insert.take() {
                self.output_port_insert.push_data(Ok(insert_data));
                pushed_something = true
            }
        }

        if pushed_something {
            Ok(Event::NeedConsume)
        } else {
            if self.input_port.has_data() {
                if self.output_data_update.is_none()
                    && self.output_data_insert.is_none()
                    && self.output_data_delete.is_none()
                {
                    self.input_data = Some(self.input_port.pull_data().unwrap()?);
                    Ok(Event::Sync)
                } else {
                    Ok(Event::NeedConsume)
                }
            } else {
                self.input_port.set_need_data();
                Ok(Event::NeedData)
            }
        }
    }

    fn process(&mut self) -> Result<()> {
        if let Some(data_block) = self.input_data.take() {
            //  we receive a partial unmodified block data. please see details at the top of this file.
            if data_block.get_meta().is_some() {
                let meta_index = BlockMetaIndex::downcast_ref_from(data_block.get_meta().unwrap());
                if meta_index.is_some() {
                    // we reserve the meta in data_block to avoid adding insert `merge_status` in `merge_into_not_matched` by mistake.
                    // if `is_empty`, it's a whole block matched, we need to delete.
                    if !data_block.is_empty() {
                        self.output_data_delete = Some(data_block.clone());
                    }
                    // if the downstream receives this, it should just treat this as a DeletedLog.
                    self.output_data_delete = Some(DataBlock::empty_with_meta(Box::new(
                        meta_index.unwrap().clone(),
                    )));
                    return Ok(());
                }
            }

            let start = Instant::now();
            let (matched_block, insert_block) = self
                .merge_into_split_mutator
                .split_data_block(&data_block)?;
            let elapsed_time = start.elapsed().as_millis() as u64;
            metrics_inc_merge_into_split_milliseconds(elapsed_time);

            if !matched_block.is_empty() {
                self.split_matched_data(matched_block)?;
            }

            if !insert_block.is_empty() {
                self.output_data_insert = Some(insert_block);
            }
        }
        Ok(())
    }
}
