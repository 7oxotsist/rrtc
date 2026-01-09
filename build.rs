fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .file_descriptor_set_path("src/file_descriptor_set.bin")  // путь к бинарнику
        .compile_protos(&["proto/sfu.proto"], &["proto"])?;
    Ok(())
}

/*tonic_prost_build::configure()
    .out_dir("/proto/sfu")
    .compile_protos(
      &["proto/sfu.proto"],
      &["proto/sfu"])?;

*/
