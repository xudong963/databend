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

use common_exception::ErrorCode;
use common_exception::Result;
use common_expression::types::string::StringColumnBuilder;
use common_expression::types::DataType;
use common_expression::types::NumberDataType;
use common_expression::types::UInt64Type;
use common_expression::BlockEntry;
use common_expression::BlockMetaInfo;
use common_expression::BlockMetaInfoDowncast;
use common_expression::BlockMetaInfoPtr;
use common_expression::ColumnId;
use common_expression::FromData;
use common_expression::Scalar;
use common_expression::TableDataType;
use common_expression::Value;
use common_expression::BLOCK_NAME_COLUMN_ID;
use common_expression::ROW_ID_COLUMN_ID;
use common_expression::SEGMENT_NAME_COLUMN_ID;
use common_expression::SNAPSHOT_NAME_COLUMN_ID;

// Segment and Block id Bits when generate internal column `_row_id`
// Since `DEFAULT_BLOCK_PER_SEGMENT` is 1000, so `block_id` 10 bits is enough.
pub const NUM_BLOCK_ID_BITS: usize = 10;
pub const NUM_SEGMENT_ID_BITS: usize = 22;
pub const NUM_ROW_ID_PREFIX_BITS: usize = NUM_BLOCK_ID_BITS + NUM_SEGMENT_ID_BITS;

#[inline(always)]
pub fn compute_row_id_prefix(seg_id: u64, block_id: u64) -> u64 {
    // `seg_id` is the offset in the segment list in the snapshot meta.
    // The bigger the `seg_id`, the older the segment.
    // So, to make the row id monotonic increasing, we need to reverse the `seg_id`.
    let seg_id = (!seg_id) & ((1 << NUM_SEGMENT_ID_BITS) - 1);
    let block_id = block_id & ((1 << NUM_BLOCK_ID_BITS) - 1);
    ((seg_id << NUM_BLOCK_ID_BITS) | block_id) & ((1 << NUM_ROW_ID_PREFIX_BITS) - 1)
}

#[inline(always)]
pub fn compute_row_id(prefix: u64, idx: u64) -> u64 {
    (prefix << NUM_ROW_ID_PREFIX_BITS) | (idx & ((1 << NUM_ROW_ID_PREFIX_BITS) - 1))
}

#[inline(always)]
pub fn split_row_id(id: u64) -> (u64, u64) {
    let prefix = id >> NUM_ROW_ID_PREFIX_BITS;
    let idx = id & ((1 << NUM_ROW_ID_PREFIX_BITS) - 1);
    (prefix, idx)
}

pub fn split_prefix(id: u64) -> (u64, u64) {
    let block_id = id & ((1 << NUM_BLOCK_ID_BITS) - 1);

    let seg_id = id >> NUM_BLOCK_ID_BITS;
    let seg_id = (!seg_id) & ((1 << NUM_SEGMENT_ID_BITS) - 1);
    (seg_id, block_id)
}

#[inline(always)]
pub fn block_id_in_segment(block_num: usize, block_idx: usize) -> usize {
    block_num - block_idx - 1
}

#[inline(always)]
pub fn block_idx_in_segment(block_num: usize, block_id: usize) -> usize {
    block_num - (block_id + 1)
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum InternalColumnMeta {
    RowId(RowIdMeta),
    // block_location: String,
    BlockName(String),
    // segment_location: String,
    SegmentName(String),
    // snapshot_location: String,
    SnapshotName(String),
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct RowIdMeta {
    pub segment_idx: usize,
    pub block_id: usize,
    pub offsets: Option<Vec<usize>>,
}

#[typetag::serde(name = "internal_column_meta")]
impl BlockMetaInfo for InternalColumnMeta {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn equals(&self, info: &Box<dyn BlockMetaInfo>) -> bool {
        match InternalColumnMeta::downcast_ref_from(info) {
            None => false,
            Some(other) => self == other,
        }
    }

    fn clone_self(&self) -> Box<dyn BlockMetaInfo> {
        Box::new(self.clone())
    }
}

impl InternalColumnMeta {
    pub fn from_meta(info: &BlockMetaInfoPtr) -> Result<&InternalColumnMeta> {
        match InternalColumnMeta::downcast_ref_from(info) {
            Some(part_ref) => Ok(part_ref),
            None => Err(ErrorCode::Internal(
                "Cannot downcast from BlockMetaInfo to InternalColumnMeta.",
            )),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub enum InternalColumnType {
    RowId,
    BlockName,
    SegmentName,
    SnapshotName,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct InternalColumn {
    pub column_name: String,
    pub column_type: InternalColumnType,
}

impl InternalColumn {
    pub fn new(name: &str, column_type: InternalColumnType) -> Self {
        InternalColumn {
            column_name: name.to_string(),
            column_type,
        }
    }

    pub fn column_type(&self) -> &InternalColumnType {
        &self.column_type
    }

    pub fn table_data_type(&self) -> TableDataType {
        match &self.column_type {
            InternalColumnType::RowId => TableDataType::Number(NumberDataType::UInt64),
            InternalColumnType::BlockName => TableDataType::String,
            InternalColumnType::SegmentName => TableDataType::String,
            InternalColumnType::SnapshotName => TableDataType::String,
        }
    }

    pub fn data_type(&self) -> DataType {
        let t = &self.table_data_type();
        t.into()
    }

    pub fn column_name(&self) -> &String {
        &self.column_name
    }

    pub fn column_id(&self) -> ColumnId {
        match &self.column_type {
            InternalColumnType::RowId => ROW_ID_COLUMN_ID,
            InternalColumnType::BlockName => BLOCK_NAME_COLUMN_ID,
            InternalColumnType::SegmentName => SEGMENT_NAME_COLUMN_ID,
            InternalColumnType::SnapshotName => SNAPSHOT_NAME_COLUMN_ID,
        }
    }

    pub fn generate_block_name_column_values(&self, block_location: &str) -> BlockEntry {
        let mut builder = StringColumnBuilder::with_capacity(1, block_location.len());
        builder.put_str(block_location);
        builder.commit_row();
        BlockEntry::new(
            DataType::String,
            Value::Scalar(Scalar::String(builder.build_scalar())),
        )
    }

    pub fn generate_segment_name_column_values(&self, segment_location: &str) -> BlockEntry {
        let mut builder = StringColumnBuilder::with_capacity(1, segment_location.len());
        builder.put_str(segment_location);
        builder.commit_row();
        BlockEntry::new(
            DataType::String,
            Value::Scalar(Scalar::String(builder.build_scalar())),
        )
    }

    pub fn generate_snapshot_name_column_values(&self, snapshot_location: &str) -> BlockEntry {
        let mut builder = StringColumnBuilder::with_capacity(1, snapshot_location.len());
        builder.put_str(snapshot_location);
        builder.commit_row();
        BlockEntry::new(
            DataType::String,
            Value::Scalar(Scalar::String(builder.build_scalar())),
        )
    }

    pub fn generate_row_id_column_values(&self, meta: &RowIdMeta, num_rows: usize) -> BlockEntry {
        let block_id = meta.block_id as u64;
        let seg_id = meta.segment_idx as u64;
        let high_32bit = compute_row_id_prefix(seg_id, block_id);
        let mut row_ids = Vec::with_capacity(num_rows);
        if let Some(offsets) = &meta.offsets {
            for i in offsets {
                let row_id = compute_row_id(high_32bit, *i as u64);
                row_ids.push(row_id);
            }
        } else {
            for i in 0..num_rows {
                let row_id = compute_row_id(high_32bit, i as u64);
                row_ids.push(row_id);
            }
        }

        BlockEntry::new(
            DataType::Number(NumberDataType::UInt64),
            Value::Column(UInt64Type::from_data(row_ids)),
        )
    }
}

impl TryFrom<InternalColumnMeta> for RowIdMeta {
    type Error = ErrorCode;

    fn try_from(value: InternalColumnMeta) -> Result<Self> {
        match value {
            InternalColumnMeta::RowId(meta) => Ok(meta),
            _ => Err(ErrorCode::Internal(format!(
                "Cannot convert {:?} to RowIdMeta.",
                value
            ))),
        }
    }
}

impl TryFrom<InternalColumnMeta> for String {
    type Error = ErrorCode;

    fn try_from(value: InternalColumnMeta) -> Result<Self> {
        match value {
            InternalColumnMeta::BlockName(block_location) => Ok(block_location),
            InternalColumnMeta::SegmentName(segment_location) => Ok(segment_location),
            InternalColumnMeta::SnapshotName(snapshot_location) => Ok(snapshot_location),
            _ => Err(ErrorCode::Internal(format!(
                "Cannot convert {:?} to String.",
                value
            ))),
        }
    }
}
