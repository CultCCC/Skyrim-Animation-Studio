//! `nifextra <file.nif>...`: print string / strings extra data (Prn, AttachT) and which node owns them.
use aether_nif::typed::controllers::{NiStringExtraData, NiStringsExtraData};
use aether_nif::typed::{Ctx, TypedBlock};
fn main() {
    for p in std::env::args().skip(1) {
        let f = aether_nif::NifFile::parse(&std::fs::read(&p).unwrap()).unwrap();
        let ctx = Ctx::of(&f.header).unwrap();
        println!("{p}");
        for (i, b) in f.blocks.iter().enumerate() {
            if let Ok(e) = NiStringExtraData::decode(b, ctx) {
                println!("  [{i}] {} = {}", e.name.resolve(&f.header).unwrap_or_default(), e.value.resolve(&f.header).unwrap_or_default());
            }
            if let Ok(e) = NiStringsExtraData::decode(b, ctx) {
                println!("  [{i}] {} = {:?}", e.name.resolve(&f.header).unwrap_or_default(), e.values);
            }
        }
    }
}
