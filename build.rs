
use std::{
    collections::BTreeMap, env, fmt::Write, fs, io, path::{Path, PathBuf}
};

use miden_assembly::{
    diagnostics::{IntoDiagnostic, Result}, utils::Serializable, Assembler, Library, LibraryNamespace, Report
};
use miden_lib::transaction::TransactionKernel;
use regex::Regex;
use walkdir::WalkDir;

// CONSTANTS
// ================================================================================================

const CAN_WRITE_TO_SRC: bool = option_env!("DOCS_RS").is_none();

const ASSETS_DIR: &str = "assets";
const ASM_DIR: &str = "asm";
const ASM_NOTE_SCRIPTS_DIR: &str = "note_scripts";
const ASM_CONTRACTS_DIR: &str = "contracts";
const NOTE_ERRORS_FILE: &str = "src/errors/note_errors.rs";
const ACCOUNT_ERRORS_FILE: &str = "src/errors/account_errors.rs";

// PRE-PROCESSING
// ================================================================================================

/// Read and parse the contents from `./asm`.
/// - Compiles contents of asm/miden directory into a Miden library file (.masl) under miden
///   namespace.
/// - Compiles contents of asm/scripts directory into individual .masb files.
fn main() -> Result<()> {
    // re-build when the MASM code changes
    println!("cargo:rerun-if-changed={ASM_DIR}");
    
    // Copies the MASM code to the build directory
    let crate_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let build_dir = env::var("OUT_DIR").unwrap();
    let src = Path::new(&crate_dir).join("src").join(ASM_DIR);
    let dst = Path::new(&build_dir).to_path_buf();
    copy_directory(src, &dst);

    // set source directory to {OUT_DIR}/asm
    let source_dir = dst.join(ASM_DIR);
    let contracts_dir = source_dir.join(ASM_CONTRACTS_DIR);

    // set target directory to {OUT_DIR}/assets
    let target_dir = Path::new(&build_dir).join(ASSETS_DIR);
    let target_contracts_dir = target_dir.join(ASM_CONTRACTS_DIR);

    // compile contracts
    let assembler = compile_contracts(&contracts_dir, &target_contracts_dir)?;

    // compile note scripts
    compile_note_scripts(
        &source_dir.join(ASM_NOTE_SCRIPTS_DIR),
        &target_dir.join(ASM_NOTE_SCRIPTS_DIR),
        assembler.clone(),
    )?;

    // Skip this build script in BUILD_NOTE_ERRORS environment variable is not set to `1`.
    if env::var("BUILD_NOTE_ERRORS").unwrap_or("0".to_string()) == "1" {
        // Generate note error constants.
        generate_note_error_constants(&source_dir.join(ASM_NOTE_SCRIPTS_DIR), NOTE_ERRORS_FILE)?;
        generate_note_error_constants(&source_dir.join(ASM_CONTRACTS_DIR), ACCOUNT_ERRORS_FILE)?;
    }


    Ok(())
}

fn create_assembler() -> Result<Assembler> {
    Ok(TransactionKernel::assembler().with_debug_mode(true))
}

// COMPILE EXECUTABLE MODULES
// ================================================================================================

/// Reads all MASM files from the "{source_dir}", complies each file individually into a MASB
/// file, and stores the complied files into the "{target_dir}".
///
/// The source files are expected to contain executable programs.
fn compile_note_scripts(source_dir: &Path, target_dir: &Path, assembler: Assembler) -> Result<()> {
    if let Err(e) = fs::create_dir_all(target_dir) {
        println!("Failed to create note_scripts directory: {}", e);
    }

    for masm_file_path in get_masm_files(source_dir).unwrap() {
        // read the MASM file, parse it, and serialize the parsed AST to bytes
        let code = assembler.clone().assemble_program(masm_file_path.clone())?;

        let bytes = code.to_bytes();

        // TODO: get rid of unwraps
        let masb_file_name = masm_file_path.file_name().unwrap().to_str().unwrap();
        let mut masb_file_path = target_dir.join(masb_file_name);

        // write the binary MASB to the output dir
        masb_file_path.set_extension("masb");
        fs::write(masb_file_path, bytes).unwrap();
    }
    Ok(())
}

const POOL_CONTRACT_CODE: &str = "
    export.::dex::pool::receive_asset
    export.::dex::pool::swap
";

fn compile_contracts(source_dir: &Path, target_dir: &Path) -> Result<Assembler> {
    if let Err(e) = fs::create_dir_all(target_dir) {
        println!("Failed to create note_scripts directory: {}", e);
    }

    let mut assembler = create_assembler()?;

    assembler.add_modules_from_dir(
        "dex".parse::<LibraryNamespace>().unwrap(), source_dir
    )?;

    for (component_name, component_code) in [
        ("pool", POOL_CONTRACT_CODE),
    ] {
        let component_library = assembler.clone().assemble_library([component_code])?;
        let component_file_path =
            target_dir.join(component_name).with_extension(Library::LIBRARY_EXTENSION);
        component_library.write_to_file(component_file_path).into_diagnostic()?;
    }

    Ok(assembler)
}

// HELPER FUNCTIONS
// ================================================================================================

/// Recursively copies `src` into `dst`.
///
/// This function will overwrite the existing files if re-executed.
fn copy_directory<T: AsRef<Path>, R: AsRef<Path>>(src: T, dst: R) {
    let mut prefix = src.as_ref().canonicalize().unwrap();
    // keep all the files inside the `asm` folder
    prefix.pop();

    let target_dir = dst.as_ref().join(ASM_DIR);
    if !target_dir.exists() {
        fs::create_dir_all(target_dir).unwrap();
    }

    let dst = dst.as_ref();
    let mut todo = vec![src.as_ref().to_path_buf()];

    while let Some(goal) = todo.pop() {
        for entry in fs::read_dir(goal).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                let src_dir = path.canonicalize().unwrap();
                let dst_dir = dst.join(src_dir.strip_prefix(&prefix).unwrap());
                if !dst_dir.exists() {
                    fs::create_dir_all(&dst_dir).unwrap();
                }
                todo.push(src_dir);
            } else {
                let dst_file = dst.join(path.strip_prefix(&prefix).unwrap());
                fs::copy(&path, dst_file).unwrap();
            }
        }
    }
}

/// Returns a vector with paths to all MASM files in the specified directory.
///
/// All non-MASM files are skipped.
fn get_masm_files<P: AsRef<Path>>(dir_path: P) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    let path = dir_path.as_ref();
    if path.is_dir() {
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(file) => {
                            let file_path = file.path();
                            if is_masm_file(&file_path)? {
                                files.push(file_path);
                            }
                        },
                        Err(e) => println!("Error reading directory entry: {}", e),
                    }
                }
            },
            Err(e) => println!("Error reading directory: {}", e),
        }
    } else {
        println!("cargo:rerun-The specified path is not a directory.");
    }

    Ok(files)
}

/// Returns true if the provided path resolves to a file with `.masm` extension.
///
/// # Errors
/// Returns an error if the path could not be converted to a UTF-8 string.
fn is_masm_file(path: &Path) -> io::Result<bool> {
    if let Some(extension) = path.extension() {
        let extension = extension
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "invalid UTF-8 filename"))?
            .to_lowercase();
        Ok(extension == "masm")
    } else {
        Ok(false)
    }
}

fn generate_note_error_constants(note_source_dir: &Path, result: &str) -> Result<()> {
    // Because the error files will be written to ./src/errors, this should be a no-op if ./src is
    // read-only
    if !CAN_WRITE_TO_SRC {
        return Ok(());
    }

    // We use a BTree here to order the errors by their categories which is the first part after the
    // ERR_ prefix and to allow for the same error code to be defined multiple times in
    // different files (as long as the constant names match).
    let mut errors = BTreeMap::new();

    // Walk all files of the kernel source directory.
    for entry in WalkDir::new(note_source_dir) {
        let entry = entry.into_diagnostic()?;
        if !is_masm_file(entry.path()).into_diagnostic()? {
            continue;
        }
        let file_contents = std::fs::read_to_string(entry.path()).into_diagnostic()?;
        extract_note_errors(&mut errors, &file_contents)?;
    }

    // Check if any error code is used twice with different error names.
    let mut error_codes = BTreeMap::new();
    for (error_name, error) in errors.iter() {
        if let Some(existing_error_name) = error_codes.get(&error.code) {
            return Err(Report::msg(format!("Note error code 0x{} is used multiple times; Non-exhaustive list: ERR_{existing_error_name}, ERR_{error_name}", error.code)));
        }

        error_codes.insert(error.code.clone(), error_name);
    }

    // Generate the errors file.
    let error_file_content = generate_note_errors(errors)?;
    std::fs::write(result, error_file_content).into_diagnostic()?;

    Ok(())
}

fn extract_note_errors(
    errors: &mut BTreeMap<ErrorName, ExtractedError>,
    file_contents: &str,
) -> Result<()> {
    let regex =
        Regex::new(r"(# (?<message>.*)\n)?const\.ERR_(?<name>.*)=0x(?<code>[\dABCDEFabcdef]*)")
            .unwrap();

    for capture in regex.captures_iter(file_contents) {
        let error_name = capture
            .name("name")
            .expect("error name should be captured")
            .as_str()
            .trim()
            .to_owned();
        let error_code = capture
            .name("code")
            .expect("error code should be captured")
            .as_str()
            .trim()
            .to_owned();

        let error_message = match capture.name("message") {
            Some(message) => message.as_str().trim().to_owned(),
            None => {
                return Err(Report::msg(format!("error message for constant ERR_{error_name} not found; add a comment above the constant to add an error message")));
            },
        };

        if let Some(ExtractedError { code: existing_error_code, .. }) = errors.get(&error_name) {
            if existing_error_code != &error_code {
                return Err(Report::msg(format!("Note error constant ERR_{error_name} is already defined elsewhere but its error code is different")));
            }
        }

        errors.insert(error_name, ExtractedError { code: error_code, message: error_message });
    }

    Ok(())
}

fn is_new_error_category<'a>(last_error: &mut Option<&'a str>, current_error: &'a str) -> bool {
    let is_new = match last_error {
        Some(last_err) => {
            let last_category =
                last_err.split("_").next().expect("there should be at least one entry");
            let new_category =
                current_error.split("_").next().expect("there should be at least one entry");
            last_category != new_category
        },
        None => false,
    };

    last_error.replace(current_error);

    is_new
}

fn generate_note_errors(errors: BTreeMap<ErrorName, ExtractedError>) -> Result<String> {
    let mut output = String::new();

    writeln!(
        output,
        "// This file is generated by build.rs, do not modify manually.
// It is generated by extracting errors from the masm files in the `miden-lib/asm` directory.
//
// To add a new error, define a constant in masm of the pattern `const.ERR_<CATEGORY>_...`.
// Try to fit the error into a pre-existing category if possible (e.g. Account, Prologue,
// Non-Fungible-Asset, ...).
//
// The comment directly above the constant will be interpreted as the error message for that error.

// NOTE ASSERTION ERROR
// ================================================================================================
"
    )
    .into_diagnostic()?;

    let mut last_error = None;
    for (error_name, ExtractedError { code, .. }) in errors.iter() {
        // Group errors into blocks separate by newlines.
        if is_new_error_category(&mut last_error, error_name) {
            writeln!(output).into_diagnostic()?;
        }
        writeln!(output, "pub const ERR_{error_name}: u32 = 0x{code};").into_diagnostic()?;
    }
    writeln!(output).into_diagnostic()?;

    writeln!(output, "pub const NOTE_ERRORS: [(u32, &str); {}] = [", errors.len())
        .into_diagnostic()?;

    let mut last_error = None;
    for (error_name, ExtractedError { message, .. }) in errors.iter() {
        // Group errors into blocks separate by newlines.
        if is_new_error_category(&mut last_error, error_name) {
            writeln!(output).into_diagnostic()?;
        }
        writeln!(output, r#"    (ERR_{error_name}, "{message}"),"#).into_diagnostic()?;
    }

    writeln!(output, "];").into_diagnostic()?;

    Ok(output)
}

type ErrorName = String;

struct ExtractedError {
    code: String,
    message: String,
}