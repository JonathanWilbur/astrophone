fn main() -> Result<(), Box<dyn std::error::Error>> {
    prost_build::compile_protos(&[
        "../proto/asn1.proto",
        "../proto/mtsid.proto",
        "../proto/callsig.proto",
        "../proto/mediacontrol.proto",
    ], &["../proto"])?;
    Ok(())
}
