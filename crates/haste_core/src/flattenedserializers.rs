use std::array;
use std::collections::hash_map;
use std::fmt;
use std::hash::BuildHasherDefault;
use std::rc::Rc;

use mars::fxhash;
use mars::nohash::NoHashMap;
use mars::varint;
use haste_protos::{
    CDemoSendTables, CsvcMsgFlattenedSerializer, ProtoFlattenedSerializerFieldT,
    ProtoFlattenedSerializerT,
};
use haste_protos::prost::{self, Message};

use crate::fieldmetadata::{
    FieldMetadata, FieldMetadataError, FieldSpecialDescriptor, get_field_metadata,
};

// NOTE: max known count of send node parts is 3.
//   you can re-calculate it with tools/uniquetypes/src/main.rs.
const MAX_SEND_NODE_PARTS: usize = 3;

#[derive(thiserror::Error, Debug)]
pub enum FlattenedSerializersError {
    #[error(transparent)]
    DecodeError(#[from] prost::DecodeError),
    #[error(transparent)]
    ReadVarintError(#[from] varint::ReadVarintError),
    /// every field whose type haste can't read, all collected in one pass.
    #[error("{}", UnsupportedFieldTypesDisplay { build: *build, fields })]
    UnsupportedFieldTypes {
        /// the replay's build number (from the file header), if known.
        build: Option<i32>,
        fields: Vec<UnsupportedFieldType>,
    },
}

impl FlattenedSerializersError {
    /// sets the build number on [`Self::UnsupportedFieldTypes`]; other errors are unchanged.
    pub(crate) fn with_build(mut self, build_num: Option<i32>) -> Self {
        if let Self::UnsupportedFieldTypes { build, .. } = &mut self {
            *build = build_num;
        }
        self
    }
}

/// a field whose type string haste can't read or has no decoder for.
#[derive(Debug, Clone)]
pub struct UnsupportedFieldType {
    /// the serializer (entity class) that first used the field, e.g. `CCitadelEventState`.
    pub class: String,
    /// e.g. `m_vecActiveEvents`.
    pub field: String,
    /// the full type string, e.g. `CNetworkUtlVectorBase< ... >`.
    pub var_type: String,
    /// why it failed.
    pub reason: String,
}

impl fmt::Display for UnsupportedFieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}: type `{}`: {}",
            self.class, self.field, self.var_type, self.reason
        )
    }
}

struct UnsupportedFieldTypesDisplay<'a> {
    build: Option<i32>,
    fields: &'a [UnsupportedFieldType],
}

impl fmt::Display for UnsupportedFieldTypesDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} field type(s) could not be read", self.fields.len())?;
        match self.build {
            Some(build) => write!(f, " (build {build})")?,
            None => write!(f, " (build unknown)")?,
        }
        for field in self.fields {
            write!(f, "\n  {field}")?;
        }
        Ok(())
    }
}

// TODO: symbol table / string cache (but do not use servo's string cache
// because it's super slow; it relies on rust-phf that uses sip13 cryptograpgic
// hasher, and you can't replace it with something else (without forking it
// really)).
//
// valve's implementation: public/tier1/utlsymbol.h;
// blukai's 3head implementation:
#[derive(Debug, Clone, Default)]
pub struct Symbol {
    pub hash: u64,
    // TODO: consider renaming str into boxed_str
    #[cfg(feature = "preserve-metadata")]
    pub str: Box<str>,
}

impl<T: AsRef<str>> From<T> for Symbol {
    #[inline(always)]
    fn from(value: T) -> Self {
        let s = value.as_ref();
        Self {
            hash: fxhash::hash_bytes(s.as_bytes()),
            #[cfg(feature = "preserve-metadata")]
            // TODO(blukai): is this the most efficient way of turning str into Box<str>?
            str: s.to_string().into_boxed_str(),
        }
    }
}

// some info about string tables
// https://developer.valvesoftware.com/wiki/Networking_Events_%26_Messages
// https://developer.valvesoftware.com/wiki/Networking_Entities

// TODO: do not clone strings, but reference them instead -> introduce lifetimes
// or build a symbol table from symbols (string cache?)

/// note about missing `field_serializer_version` field (from
/// [`haste_protos::ProtoFlattenedSerializerFieldT`]): i did not find any evidence of it
/// being used nor any breakage or data corruptions. it is possible that i missed something. but
/// unless proven otherwise i don't see a reason for incorporating it. field serializers always
/// reference "highest" version of serializer.
///
/// it might be reasonable to actually incorporate it if there will be a need to run this parser in
/// an environment that processes high volumes of replays. theoretically flattened serializers can
/// be parsed once and then reused for future parse passes.
#[derive(Debug, Clone, Default)]
pub struct FlattenedSerializerField {
    pub var_type: Symbol,
    pub var_name: Symbol,
    pub bit_count: Option<i32>,
    pub low_value: Option<f32>,
    pub high_value: Option<f32>,
    pub encode_flags: Option<i32>,
    pub field_serializer_name: Option<Symbol>,
    pub send_node: Option<[Option<Symbol>; MAX_SEND_NODE_PARTS]>,
    pub var_encoder: Option<Symbol>,

    pub field_serializer: Option<Rc<FlattenedSerializer>>,
    pub(crate) metadata: FieldMetadata,
}

/// the symbol at `index`, which must be in the message's symbol table.
fn symbol_at<'a>(
    msg: &'a CsvcMsgFlattenedSerializer,
    symbol: &'static str,
    index: i32,
) -> Result<&'a String, FieldMetadataError> {
    usize::try_from(index)
        .ok()
        .and_then(|i| msg.symbols.get(i))
        .ok_or(FieldMetadataError::SymbolIndexOutOfRange {
            symbol,
            index,
            len: msg.symbols.len(),
        })
}

fn required_symbol<'a>(
    msg: &'a CsvcMsgFlattenedSerializer,
    symbol: &'static str,
    index: Option<i32>,
) -> Result<&'a String, FieldMetadataError> {
    let index = index.ok_or(FieldMetadataError::MissingSymbol { symbol })?;
    symbol_at(msg, symbol, index)
}

fn optional_symbol<'a>(
    msg: &'a CsvcMsgFlattenedSerializer,
    symbol: &'static str,
    index: Option<i32>,
) -> Result<Option<&'a String>, FieldMetadataError> {
    index.map(|i| symbol_at(msg, symbol, i)).transpose()
}

// TODO: try to split flattened serializer field initialization into 3 clearly separate stages
// (protobuf mapping; metadata; field serializer construction).
impl FlattenedSerializerField {
    pub(crate) fn new(
        msg: &CsvcMsgFlattenedSerializer,
        field: &ProtoFlattenedSerializerFieldT,
    ) -> Result<Self, FieldMetadataError> {
        // the type and the name are required; the other symbols are optional, but any symbol
        // that is present must point into `msg.symbols`.
        let var_type = required_symbol(msg, "var_type_sym", field.var_type_sym)?;
        let var_type_symbol = Symbol::from(var_type);

        let var_name = required_symbol(msg, "var_name_sym", field.var_name_sym)?;
        let var_name_symbol = Symbol::from(var_name);

        // NOTE(blukai): send node is like a path for a field.
        //   the field itself is from a different struct that is embedded into this one.
        //   seems to be a result of `CNetworkVarEmbedded`, `SENDINFO_STRUCTELEM`, macro work:
        //     - `CNetworkVarEmbedded( fogparams_t, m_fog )`
        //     - `SendPropFloat( SENDINFO_STRUCTELEM( fogparams_t, m_fog, start ), 0, SPROP_NOSCALE )`
        //     - `SendPropDataTable( SENDINFO_DT( m_AttributeManager ), &REFERENCE_SEND_TABLE(DT_AttributeManager) )`
        //
        //   deadlock's CCitadelPlayerPawn entity has two m_nHeroID fields.
        //   send_node allows to differentiate them:
        //     - m_CCitadelHeroComponent.m_spawnedHero.m_nHeroID
        //     - m_CCitadelHeroComponent.m_loadingHero.m_nHeroID
        //     where `m_CCitadelHeroComponent.*` is `send_node` and `m_nHeroID` is `var_name`.
        //
        //   examples:
        //     - `m_CCitadelHeroComponent.m_loadingHero`
        //     - `m_CCitadelHeroComponent`
        //     - `m_CHitboxComponent`
        //     - `m_skybox3d.fog`
        //
        //   each component needs to be hashed separately, to be consistent with what
        //   `fkey_from_path` does.
        let send_node = match optional_symbol(msg, "send_node_sym", field.send_node_sym)? {
            Some(send_node) if !send_node.is_empty() => {
                let mut ret: [Option<Symbol>; MAX_SEND_NODE_PARTS] = array::from_fn(|_| None);
                for (i, part) in send_node.split('.').enumerate() {
                    ret[i] = Some(Symbol::from(part));
                }
                Some(ret)
            }
            _ => None,
        };

        let mut ret = Self {
            var_type: var_type_symbol,
            var_name: var_name_symbol,
            bit_count: field.bit_count,
            low_value: field.low_value,
            high_value: field.high_value,
            encode_flags: field.encode_flags,
            field_serializer_name: optional_symbol(
                msg,
                "field_serializer_name_sym",
                field.field_serializer_name_sym,
            )?
            .map(Symbol::from),
            send_node,
            var_encoder: optional_symbol(msg, "var_encoder_sym", field.var_encoder_sym)?
                .map(Symbol::from),

            field_serializer: None,
            metadata: Default::default(),
        };
        ret.metadata = get_field_metadata(&ret, var_type)?;
        Ok(ret)
    }

    #[inline(always)]
    pub(crate) unsafe fn get_child_unchecked(&self, index: usize) -> &Self {
        let fs = self.field_serializer.as_ref();

        debug_assert!(fs.is_some(), "field serializer is missing");

        fs.unwrap_unchecked().get_child_unchecked(index)
    }

    // NOTE: using this method can hurt performance when used in critical code
    // paths. use the unsafe [`Self::get_child_unchecked`] instead.
    pub fn get_child(&self, index: usize) -> Option<&Self> {
        self.field_serializer
            .as_ref()
            .and_then(|fs| fs.get_child(index))
    }

    #[inline(always)]
    pub(crate) fn var_encoder_heq(&self, rhs: u64) -> bool {
        self.var_encoder.as_ref().is_some_and(|lhs| lhs.hash == rhs)
    }

    #[inline(always)]
    pub fn is_dynamic_array(&self) -> bool {
        self.metadata
            .special_descriptor
            .as_ref()
            .is_some_and(|sd| sd.is_dynamic_array())
    }
}

/// note about missing `serializer_version` field (from
/// [`haste_protos::ProtoFlattenedSerializerT`]): entities resolve their serializers by
/// looking up their class info within the [crate::entityclasses::EntityClasses] struct (which i
/// parse out of [`haste_protos::CDemoClassInfo`] proto).
/// [`haste_protos::CDemoClassInfo`] carries absolutely no info about serializer version
/// thus i don't see any need to preserve it.
//
// NOTE: Clone is derived because Entity in entities.rs needs to be clonable which means that all
// members of it also should be clonable.
//
// TODO: why does FlattnedeSerializer need to derive Default?
#[derive(Debug, Clone, Default)]
pub struct FlattenedSerializer {
    pub serializer_name: Symbol,
    pub fields: Vec<Rc<FlattenedSerializerField>>,
}

impl FlattenedSerializer {
    fn new(msg: &CsvcMsgFlattenedSerializer, fs: &ProtoFlattenedSerializerT) -> Self {
        // SAFETY: some symbols are cricual, if they don't exist - fail early
        // and loudly.
        let resolve_sym_unchecked = |i: i32| unsafe { msg.symbols.get_unchecked(i as usize) };

        let serializer_name = unsafe {
            fs.serializer_name_sym
                .map(resolve_sym_unchecked)
                .unwrap_unchecked()
        };

        Self {
            serializer_name: Symbol::from(serializer_name),
            fields: Vec::with_capacity(fs.fields_index.len()),
        }
    }

    #[inline(always)]
    pub(crate) unsafe fn get_child_unchecked(&self, index: usize) -> &FlattenedSerializerField {
        debug_assert!(
            self.fields.get(index).is_some(),
            "field at index {} is missing",
            index,
        );

        self.fields.get_unchecked(index)
    }

    // NOTE: using this method can hurt performance when used in critical code
    // paths. use the unsafe [`Self::get_child_unchecked`] instead.
    pub fn get_child(&self, index: usize) -> Option<&FlattenedSerializerField> {
        self.fields.get(index).map(|field| field.as_ref())
    }
}

pub struct FlattenedSerializerContainer {
    serializer_map: NoHashMap<u64, Rc<FlattenedSerializer>>,
}

impl FlattenedSerializerContainer {
    pub fn parse(cmd: CDemoSendTables) -> Result<Self, FlattenedSerializersError> {
        let msg = {
            // TODO: make prost work with ByteString and turn data into Bytes
            //
            // NOTE: calling unwrap_or_default is for some reason faster then
            // relying on prost's default unwrapping by calling .data().
            let mut data = &cmd.data.unwrap_or_default()[..];
            // NOTE: count is useless because read_uvarint32 will "consume"
            // bytes that it'll read from data; size is useless because data
            // supposedly contains only one message.
            //
            // NOTE: there are 2 levels of indirection here, but rust's compiler
            // may optimize them away. but if this will be affecting performance
            // -> createa a function that will be capable of reading varint from
            // &[u8] without multiple levels of indirection.
            let (_size, _count) = varint::read_uvarint64(&mut data)?;
            CsvcMsgFlattenedSerializer::decode(data)?
        };

        let mut field_map: NoHashMap<i32, Rc<FlattenedSerializerField>> =
            NoHashMap::with_capacity_and_hasher(msg.fields.len(), BuildHasherDefault::default());
        let mut serializer_map: NoHashMap<u64, Rc<FlattenedSerializer>> =
            NoHashMap::with_capacity_and_hasher(
                msg.serializers.len(),
                BuildHasherDefault::default(),
            );

        let mut unsupported: Vec<UnsupportedFieldType> = Vec::new();
        let mut failed_fields: std::collections::HashSet<i32> = Default::default();

        for serializer in msg.serializers.iter() {
            let mut flattened_serializer = FlattenedSerializer::new(&msg, serializer);

            for field_index in serializer.fields_index.iter() {
                if let Some(field) = field_map.get(field_index) {
                    flattened_serializer.fields.push(field.clone());
                    continue;
                }

                let proto_field = &msg.fields[*field_index as usize];
                let mut field = match FlattenedSerializerField::new(&msg, proto_field) {
                    Ok(field) => field,
                    Err(err) => {
                        // keep going, so every unsupported type is reported at once. a field
                        // shared by several serializers is reported once, with the first.
                        if failed_fields.insert(*field_index) {
                            let sym = |i: Option<i32>| {
                                i.and_then(|i| msg.symbols.get(i as usize))
                                    .map_or_else(|| "?".to_string(), |s| s.to_string())
                            };
                            unsupported.push(UnsupportedFieldType {
                                class: sym(serializer.serializer_name_sym),
                                field: sym(proto_field.var_name_sym),
                                var_type: sym(proto_field.var_type_sym),
                                reason: err.to_string(),
                            });
                        }
                        continue;
                    }
                };

                field.field_serializer = match field.metadata.special_descriptor {
                    Some(FieldSpecialDescriptor::FixedArray { length }) => {
                        let mut field = field.clone();
                        field.field_serializer = field
                            .field_serializer_name
                            .as_ref()
                            .and_then(|symbol| serializer_map.get(&symbol.hash).cloned());
                        Some(Rc::new(FlattenedSerializer {
                            fields: {
                                let mut fields = Vec::with_capacity(length);
                                fields.resize(length, Rc::new(field));
                                fields
                            },
                            ..Default::default()
                        }))
                    }
                    Some(FieldSpecialDescriptor::DynamicArray { ref decoder }) => {
                        let field = FlattenedSerializerField {
                            metadata: FieldMetadata {
                                decoder: decoder.clone(),
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        Some(Rc::new(FlattenedSerializer {
                            fields: vec![Rc::new(field)],
                            ..Default::default()
                        }))
                    }
                    Some(FieldSpecialDescriptor::DynamicSerializerArray) => {
                        let field = FlattenedSerializerField {
                            field_serializer: field
                                .field_serializer_name
                                .as_ref()
                                .and_then(|symbol| serializer_map.get(&symbol.hash).cloned()),
                            ..Default::default()
                        };
                        Some(Rc::new(FlattenedSerializer {
                            fields: vec![Rc::new(field)],
                            ..Default::default()
                        }))
                    }
                    _ => field
                        .field_serializer_name
                        .as_ref()
                        .and_then(|symbol| serializer_map.get(&symbol.hash).cloned()),
                };

                let field = Rc::new(field);
                field_map.insert(*field_index, field.clone());
                flattened_serializer.fields.push(field);
            }

            serializer_map.insert(
                flattened_serializer.serializer_name.hash,
                Rc::new(flattened_serializer),
            );
        }

        if !unsupported.is_empty() {
            return Err(FlattenedSerializersError::UnsupportedFieldTypes {
                build: None,
                fields: unsupported,
            });
        }

        Ok(Self { serializer_map })
    }

    // TODO: think about exposing the whole serializer map

    #[inline(always)]
    pub fn by_name_hash(&self, serializer_name_hash: u64) -> Option<Rc<FlattenedSerializer>> {
        self.serializer_map.get(&serializer_name_hash).cloned()
    }

    #[inline(always)]
    pub unsafe fn by_name_hash_unckecked(
        &self,
        serializer_name_hash: u64,
    ) -> Rc<FlattenedSerializer> {
        self.serializer_map
            .get(&serializer_name_hash)
            .unwrap_unchecked()
            // NOTE: do not chain .cloned() after calling .get(), because .cloned() uses match
            // under the hood which adds a branch; that is redunant.
            .clone()
    }

    #[inline]
    pub fn values(&self) -> hash_map::Values<'_, u64, Rc<FlattenedSerializer>> {
        self.serializer_map.values()
    }
}

#[cfg(test)]
mod tests {
    use haste_protos::{
        CDemoSendTables, CsvcMsgFlattenedSerializer, ProtoFlattenedSerializerFieldT,
        ProtoFlattenedSerializerT,
    };
    use haste_protos::prost::Message;

    use super::*;

    // a send table with one good field, one field no decoder handles, and one that does not
    // parse; the last is shared by two serializers.
    fn send_tables() -> CDemoSendTables {
        let symbols = [
            "CFoo", "CBar", "m_ok", "int32", "m_bad", "CUtlVector< 5 >", "m_shared", "A<>",
        ];
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
            fields: vec![field(2, 3), field(4, 5), field(6, 7)],
            serializers: vec![serializer(0, vec![0, 1, 2]), serializer(1, vec![2])],
            ..Default::default()
        };
        CDemoSendTables {
            data: Some(msg.encode_length_delimited_to_vec()),
        }
    }

    #[test]
    fn unsupported_field_types_are_all_reported() {
        let Err(err) = FlattenedSerializerContainer::parse(send_tables()) else {
            panic!("expected an error");
        };
        let err = err.with_build(Some(6866));
        assert_eq!(
            err.to_string(),
            "2 field type(s) could not be read (build 6866)\n  \
             CFoo.m_bad: type `CUtlVector< 5 >`: unsupported type expression: \
             Num(5) where a type is expected\n  \
             CFoo.m_shared: type `A<>`: unexpected token at 2"
        );
    }

    fn new_field(
        symbols: &[&str],
        mutate: impl FnOnce(&mut ProtoFlattenedSerializerFieldT),
    ) -> Result<FlattenedSerializerField, FieldMetadataError> {
        let mut field = ProtoFlattenedSerializerFieldT {
            var_type_sym: Some(0),
            var_name_sym: Some(1),
            ..Default::default()
        };
        mutate(&mut field);
        let msg = CsvcMsgFlattenedSerializer {
            symbols: symbols.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        FlattenedSerializerField::new(&msg, &field)
    }

    #[test]
    fn field_with_valid_symbols_is_built() {
        let field = new_field(&["int32", "m_a"], |_| {}).unwrap();
        assert_eq!(field.var_type.hash, Symbol::from("int32").hash);
        assert_eq!(field.var_name.hash, Symbol::from("m_a").hash);
    }

    #[test]
    fn field_without_type_or_name_symbol_is_an_error() {
        let err = new_field(&["int32", "m_a"], |f| f.var_type_sym = None).unwrap_err();
        assert_eq!(err.to_string(), "flattened serializer field has no var_type_sym");
        let err = new_field(&["int32", "m_a"], |f| f.var_name_sym = None).unwrap_err();
        assert_eq!(err.to_string(), "flattened serializer field has no var_name_sym");
    }

    #[test]
    fn field_with_out_of_range_symbol_is_an_error() {
        let err = new_field(&["int32", "m_a"], |f| f.var_name_sym = Some(2)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "flattened serializer field: var_name_sym is 2, but there are 2 symbols"
        );
        let err = new_field(&["int32", "m_a"], |f| f.var_type_sym = Some(-1)).unwrap_err();
        assert!(err.to_string().contains("var_type_sym is -1"), "{err}");
        // optional symbols are checked too.
        let err = new_field(&["int32", "m_a"], |f| f.send_node_sym = Some(9)).unwrap_err();
        assert!(err.to_string().contains("send_node_sym is 9"), "{err}");
    }
}
