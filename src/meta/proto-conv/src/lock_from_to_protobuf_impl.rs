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

//! This mod is the key point about compatibility.
//! Everytime update anything in this file, update the `VER` and let the tests pass.

use chrono::DateTime;
use chrono::Utc;
use common_meta_app::schema as mt;
use common_protos::pb;
use num::FromPrimitive;

use crate::reader_check_msg;
use crate::FromToProto;
use crate::Incompatible;
use crate::MIN_READER_VER;
use crate::VER;

impl FromToProto for mt::LockKey {
    type PB = pb::LockKey;
    fn get_pb_ver(p: &Self::PB) -> u64 {
        p.ver
    }

    fn from_pb(p: pb::LockKey) -> Result<Self, Incompatible>
    where Self: Sized {
        reader_check_msg(p.ver, p.min_reader_ver)?;

        match p.key {
            Some(pb::lock_key::Key::Table(pb::lock_key::Table { table_id })) => {
                Ok(mt::LockKey::Table { table_id })
            }
            None => Err(Incompatible {
                reason: "LockKey cannot be None".to_string(),
            }),
        }
    }

    fn to_pb(&self) -> Result<pb::LockKey, Incompatible> {
        let key = match self {
            mt::LockKey::Table { table_id } => {
                Some(pb::lock_key::Key::Table(pb::lock_key::Table {
                    table_id: *table_id,
                }))
            }
        };
        Ok(pb::LockKey {
            ver: VER,
            min_reader_ver: MIN_READER_VER,
            key,
        })
    }
}

impl FromToProto for mt::LockMeta {
    type PB = pb::LockMeta;
    fn get_pb_ver(p: &Self::PB) -> u64 {
        p.ver
    }

    fn from_pb(p: pb::LockMeta) -> Result<Self, Incompatible> {
        reader_check_msg(p.ver, p.min_reader_ver)?;

        let v = Self {
            user: p.user,
            node: p.node,
            session_id: p.session_id,
            created_on: DateTime::<Utc>::from_pb(p.created_on)?,
            acquired_on: match p.acquired_on {
                Some(acquired_on) => Some(DateTime::<Utc>::from_pb(acquired_on)?),
                None => None,
            },
            lock_type: FromPrimitive::from_i32(p.lock_type).ok_or_else(|| Incompatible {
                reason: format!("invalid LockType: {}", p.lock_type),
            })?,
            extra_info: p.extra_info,
        };

        Ok(v)
    }

    fn to_pb(&self) -> Result<pb::LockMeta, Incompatible> {
        let p = pb::LockMeta {
            ver: VER,
            min_reader_ver: MIN_READER_VER,
            user: self.user.clone(),
            node: self.node.clone(),
            session_id: self.session_id.clone(),
            created_on: self.created_on.to_pb()?,
            acquired_on: match self.acquired_on {
                Some(acquired_on) => Some(acquired_on.to_pb()?),
                None => None,
            },
            lock_type: self.lock_type.clone() as i32,
            extra_info: self.extra_info.clone(),
        };
        Ok(p)
    }
}