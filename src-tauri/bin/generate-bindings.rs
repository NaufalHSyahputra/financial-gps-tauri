fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../src/lib/drivers/native/bindings.ts");
    financial_gps::ipc::export_bindings(path)?;
    Ok(())
}
