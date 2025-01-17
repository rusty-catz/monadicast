use monadicast::MonadicAst;
use std::env;
use std::error::Error;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <input-directory>", args[0]);
        std::process::exit(1);
    }

    let input_dir = Path::new(&args[1]);
    if !input_dir.exists() || !input_dir.is_dir() {
        eprintln!("The specified input path is not a valid directory.");
        std::process::exit(1);
    }

    for entry in WalkDir::new(input_dir) {
        let entry = entry?;
        match entry.file_type().is_file() {
            true => {
                let input_path = entry.path();

                let content = fs::read_to_string(&input_path)?;
                let output = MonadicAst::new(&content)?
                    .convert_ffi_types()
                    .replace_raw_pointers()
                    .replace_while_loop()
                    .remove_useless_identifier_expressions()
                    .result();

                let relative_path = input_path.strip_prefix("examples")?;
                let output_path = Path::new("output").join(relative_path);

                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&output_path, output)?;
                println!("Processed: {}", input_path.display());
            }
            false => {}
        }
    }
    println!("Successfully processed all files in the directory.");
    Ok(())
}
