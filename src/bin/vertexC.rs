// NOTE:This is the main Vertex compiler CLI used until `apex` is ready for production.
// It is intended to compile a single file without external dependencies.
// Currently, it does not have a working linker. Once `apex` is ready, this tool will likely be replaced or deprecated and not be ready for
// production.
use std::env;
use tokio::runtime::Runtime;
use vertex::backend::saving_bytes::compile_tools::build_prj;
use vertex::backend::{
    errors::cli_errors::CommandLineError::{
        self, BuildHasJustTwoArg, NoFileSpecifiedForBuild, NoSuchCommand,
    },
    errors::compiler::error_explain::ERROR_EXPLAIN,
};
use vertex::runtime::runner::running_vm::run_code;

fn main() {
    if let Err(e) = run_cli() {
        eprintln!(
            "{}",
            match e {
                BuildHasJustTwoArg => "Build command has just two arguments",
                NoFileSpecifiedForBuild => "No file specified for build",
                NoSuchCommand => "No such command. Run 'vertexC help for more info'",
                _ => todo!(),
            }
        );
    }
}

fn run_cli() -> Result<(), CommandLineError> {
    let args: Vec<String> = env::args().collect();
    let rt = Runtime::new().unwrap();
    if args.len() < 2 {
        return Err(NoSuchCommand);
    }

    match args[1].as_str() {
        "build" => {
            let (debug, source, output, path_to_vm) = parse_build_args(&args[2..])?;
            rt.block_on(build_prj(source, output, debug));
            Ok(())
        }
        "run" => {
            if args.len() != 3 {
                return Err(NoFileSpecifiedForBuild);
            }
            run_code(&args[2].clone());
            Ok(())
        }
        "exec" => {
            let (debug, source, output, path_to_vm) = parse_build_args(&args[2..])?;
            rt.block_on(build_prj(source.clone(), output.clone(), debug));

            println!("\n\x1b[1;32mRunning final bytecode\x1b[0m",);
            run_code(&format!("out/{}", output));
            Ok(())
        }
        "error" => {
            if args.len() != 3 {
                eprintln!("Usage: vertexC error [ERROR_CODE]");
                return Ok(());
            }

            let code = args[2].as_str();

            match ERROR_EXPLAIN.get(code) {
                Some(text) => println!("{}", text),
                None => eprintln!("Unknown error code: {}", code),
            }

            Ok(())
        }
        "help" => {
            vertex::clrprintln!(
                r#"
$blue|vertexC$reset| — Low-level compiler tool for Vertex

$green|USAGE:$reset|
    vertexC $cyan|<COMMAND>$reset| [INPUT] [OUTPUT] [FLAGS]

$green|COMMANDS:$reset|
    $cyan|build$reset| <IN> <OUT>   Compile source file into bytecode
    $cyan|run$reset|   <BYTECODE>   Execute bytecode using the virtual machine
    $cyan|exec$reset|  <IN> <OUT>   Compile and immediately run the produced bytecode
    $cyan|error$reset| <CODE>       Detailed explanation of a specific error code
    $cyan|help$reset|               Display this help message

$green|FLAGS:$reset|
    $cyan|-d$reset|                  Show final instructions (debug)

$green|DESCRIPTION:$reset|
    vertexC compiles a single source file into Vertex bytecode. 
    It is intended for testing, debugging, and low-level workflows. 
    For project management, use the 'apex' tool.
"#
            );
            Ok(())
        }
        "build-lib" => {
            eprint!("Building vertex libs is still unimplemented");
            Ok(())
        }
        _ => Err(NoSuchCommand),
    }
}

fn parse_build_args(args: &[String]) -> Result<(bool, String, String, String), CommandLineError> {
    let mut debug = false;
    let mut source = String::new();
    let mut output = String::new();
    let mut path_to_vm = String::new();

    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-d" => {
                debug = true;
                i += 1;
            }
            _ => {
                if source.is_empty() {
                    source = args[i].clone();
                } else if output.is_empty() {
                    output = args[i].clone();
                } else if output.is_empty() {
                    path_to_vm = args[i].clone();
                } else {
                    return Err(BuildHasJustTwoArg);
                }
                i += 1;
            }
        }
    }

    if source.is_empty() || output.is_empty() {
        return Err(BuildHasJustTwoArg);
    }

    Ok((debug, source, output, path_to_vm))
}
