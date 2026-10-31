fn main() {
    // pictograma și informațiile de versiune pentru executabilul Windows
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for(
            "assets/cadastru.rc",
            ["cadastru", "cadastru-cli"],
            embed_resource::NONE,
        )
        .manifest_optional()
        .unwrap();
    }
    println!("cargo:rerun-if-changed=assets/cadastru.rc");
    println!("cargo:rerun-if-changed=assets/cadastru.ico");
}
