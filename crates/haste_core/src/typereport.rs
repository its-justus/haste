//! the "types" report: every distinct field type string in a replay, whether haste can
//! decode it, and whether it is new (not in a list of known types).
//!
//! it reads only the replay's file header and send tables, without building the entity
//! serializers, so it works on a replay that haste cannot decode. that is its purpose: when
//! a new game build adds a type haste does not handle, this lists them.

use std::collections::{BTreeMap, HashSet};

use mars::varint;
use valveprotos::common::{
    CDemoFileHeader, CDemoSendTables, CsvcMsgFlattenedSerializer, EDemoCommands,
};
use valveprotos::prost::{self, Message};

pub use haste_vartype::known_deadlock_types;

use crate::demostream::{DemoStream, ReadCmdError, ReadCmdHeaderError};
use crate::flattenedserializers::FlattenedSerializerField;

#[derive(thiserror::Error, Debug)]
pub enum TypeReportError {
    #[error(transparent)]
    ReadCmdHeader(#[from] ReadCmdHeaderError),
    #[error(transparent)]
    ReadCmd(#[from] ReadCmdError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Decode(#[from] prost::DecodeError),
    #[error(transparent)]
    ReadVarint(#[from] varint::ReadVarintError),
    #[error("no send tables before the first tick")]
    NoSendTables,
}

/// one distinct type string.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeEntry {
    /// the full type string, e.g. `CNetworkUtlVectorBase< CHandle< CBaseEntity > >`.
    pub var_type: String,
    /// `Err(reason)` if haste cannot read the type string or has no decoder for it.
    pub supported: Result<(), String>,
    /// false if the type string is not in the known list given to the report.
    pub known: bool,
    /// how many fields (class.field pairs) use it.
    pub uses: usize,
    /// the first class.field that uses it, e.g. `CCitadelEventState.m_vecActiveEvents`.
    pub first_use: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeReport {
    /// the replay's build number, from its file header.
    pub build: Option<i32>,
    /// sorted by type string.
    pub types: Vec<TypeEntry>,
}

impl TypeReport {
    pub fn new_types(&self) -> impl Iterator<Item = &TypeEntry> {
        self.types.iter().filter(|t| !t.known)
    }

    pub fn unsupported_types(&self) -> impl Iterator<Item = &TypeEntry> {
        self.types.iter().filter(|t| t.supported.is_err())
    }
}

/// reads the report from a replay. `known` is the list new types are compared against,
/// e.g. [`known_deadlock_types`].
pub fn read_type_report<D: DemoStream>(
    demo: &mut D,
    known: &HashSet<&str>,
) -> Result<TypeReport, TypeReportError> {
    let mut build = None;
    let send_tables = loop {
        if demo.is_at_eof()? {
            return Err(TypeReportError::NoSendTables);
        }
        let cmd_header = demo.read_cmd_header()?;
        match cmd_header.cmd {
            EDemoCommands::DemFileHeader => {
                build = CDemoFileHeader::decode(demo.read_cmd(&cmd_header)?)?.build_num;
            }
            EDemoCommands::DemSendTables => {
                break CDemoSendTables::decode(demo.read_cmd(&cmd_header)?)?;
            }
            _ if cmd_header.tick > 0 => return Err(TypeReportError::NoSendTables),
            _ => demo.skip_cmd(&cmd_header)?,
        }
    };

    let mut data = &send_tables.data.unwrap_or_default()[..];
    let _size = varint::read_uvarint64(&mut data)?;
    let msg = CsvcMsgFlattenedSerializer::decode(data)?;
    Ok(report_from_serializers(&msg, build, known))
}

pub(crate) fn report_from_serializers(
    msg: &CsvcMsgFlattenedSerializer,
    build: Option<i32>,
    known: &HashSet<&str>,
) -> TypeReport {
    let sym = |i: Option<i32>| {
        i.and_then(|i| msg.symbols.get(i as usize))
            .map_or_else(|| "?".to_string(), |s| s.to_string())
    };

    let mut types: BTreeMap<String, TypeEntry> = BTreeMap::new();
    for serializer in &msg.serializers {
        for field_index in &serializer.fields_index {
            let Some(field) = msg.fields.get(*field_index as usize) else {
                continue;
            };
            let var_type = sym(field.var_type_sym);
            let entry = types.entry(var_type.clone()).or_insert_with(|| TypeEntry {
                supported: FlattenedSerializerField::new(msg, field)
                    .map(|_| ())
                    .map_err(|err| err.to_string()),
                known: known.contains(var_type.as_str()),
                uses: 0,
                first_use: format!(
                    "{}.{}",
                    sym(serializer.serializer_name_sym),
                    sym(field.var_name_sym)
                ),
                var_type,
            });
            entry.uses += 1;
        }
    }

    TypeReport {
        build,
        types: types.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use valveprotos::common::{ProtoFlattenedSerializerFieldT, ProtoFlattenedSerializerT};

    use super::*;

    #[test]
    fn marks_new_and_unsupported_types() {
        let symbols = ["CFoo", "CBar", "m_a", "int32", "m_b", "CUtlVector< 5 >", "m_c"];
        let field = |name: i32, ty: i32| ProtoFlattenedSerializerFieldT {
            var_name_sym: Some(name),
            var_type_sym: Some(ty),
            ..Default::default()
        };
        let serializer = |name: i32, fields: Vec<i32>| ProtoFlattenedSerializerT {
            serializer_name_sym: Some(name),
            serializer_version: Some(0),
            fields_index: fields,
        };
        let msg = CsvcMsgFlattenedSerializer {
            symbols: symbols.iter().map(|s| s.to_string()).collect(),
            // m_a and m_c are int32; m_b is a vector of a number.
            fields: vec![field(2, 3), field(4, 5), field(6, 3)],
            serializers: vec![serializer(0, vec![0, 1]), serializer(1, vec![0, 2])],
        };
        let known = HashSet::from(["int32"]);

        let report = report_from_serializers(&msg, Some(6866), &known);

        assert_eq!(report.build, Some(6866));
        assert_eq!(report.types.len(), 2);
        let vector = &report.types[0];
        assert_eq!(vector.var_type, "CUtlVector< 5 >");
        assert!(!vector.known);
        assert!(vector.supported.is_err());
        assert_eq!((vector.uses, vector.first_use.as_str()), (1, "CFoo.m_b"));
        let int = &report.types[1];
        assert_eq!(int.var_type, "int32");
        assert!(int.known);
        assert_eq!(int.supported, Ok(()));
        // m_a in both classes, m_c once
        assert_eq!((int.uses, int.first_use.as_str()), (3, "CFoo.m_a"));
        assert_eq!(report.new_types().count(), 1);
        assert_eq!(report.unsupported_types().count(), 1);
    }
}
