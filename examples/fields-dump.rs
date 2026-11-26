//! Every field of every serializer: serializer, version, name, type, encoder, bits, flags.
use std::fs::File;
use std::io::BufReader;

use anyhow::{Context as _, Result};
use haste::demofile::DemoFile;
use haste::demostream::DemoStream;
use haste::protos::prost::Message;
use haste::protos::{CDemoSendTables, CsvcMsgFlattenedSerializer, EDemoCommands};

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: fields-dump <replay.dem>")?;
    let mut demo = DemoFile::start_reading(BufReader::new(File::open(path)?))?;
    let tables = loop {
        let h = demo.read_cmd_header()?;
        if h.cmd == EDemoCommands::DemSendTables {
            break CDemoSendTables::decode(demo.read_cmd(&h)?)?;
        }
        demo.skip_cmd(&h)?;
    };
    let data = tables.data.unwrap_or_default();
    let mut d = &data[..];
    let mut n = 0u64;
    let mut shift = 0;
    loop {
        let b = d[0];
        d = &d[1..];
        n |= ((b & 0x7f) as u64) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            break;
        }
    }
    let _ = n;
    let msg = CsvcMsgFlattenedSerializer::decode(d)?;
    let sym = |i: Option<i32>| {
        i.and_then(|i| msg.symbols.get(i as usize))
            .cloned()
            .unwrap_or_default()
    };
    println!("serializer\tversion\tname\ttype\tencoder\tbits\tflags\tlow\thigh\tfield_serializer");
    for s in &msg.serializers {
        for fi in &s.fields_index {
            let f = &msg.fields[*fi as usize];
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                sym(s.serializer_name_sym),
                s.serializer_version.unwrap_or(-1),
                sym(f.var_name_sym),
                sym(f.var_type_sym),
                sym(f.var_encoder_sym),
                f.bit_count.map_or(String::new(), |v| v.to_string()),
                f.encode_flags.map_or(String::new(), |v| v.to_string()),
                f.low_value.map_or(String::new(), |v| v.to_string()),
                f.high_value.map_or(String::new(), |v| v.to_string()),
                sym(f.field_serializer_name_sym)
            );
        }
    }
    Ok(())
}
