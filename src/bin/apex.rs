//!Main vertex package manager and linker
use serde::Deserialize;
use std::{
    collections::HashMap,
    env::{self},
    fs::{self, File, remove_dir_all},
    io::Write,
    process,
};
use tokio::runtime::Runtime;
use vertex::{
    backend::{
        errors::cli_errors::CommandLineError,
        saving_bytes::compile_tools::{build_prj, set_config},
    },
    clrprintln,
};

#[derive(Deserialize)]
struct Config {
    name: String,
    #[serde(default)]
    dependencies: HashMap<String, String>,
}
fn main() {
    if let Err(err) = run_cli() {
        eprintln!("{}", err);
    }
}

fn run_cli() -> Result<(), CommandLineError> {
    let args: Vec<String> = env::args().collect();
    let rt = Runtime::new().unwrap();

    if args.is_empty() {
        return Err(CommandLineError::InvalidCommand);
    }

    if let Some(arg1) = args.get(1) {
        match arg1.as_str() {
            "help" => {
                vertex::clrprintln!(
                    r#"
$blue|apex$reset| — The official package manager and build tool for Vertex

$green|USAGE:$reset|
    apex $cyan|<COMMAND>$reset| [FLAGS]

$green|COMMANDS:$reset|
    $cyan|new$reset| <NAME>      Create a new Vertex project with a default structure
    $cyan|build$reset|           Build the project into bytecode (placed in ./out/)
    $cyan|clear$reset|           Remove all build artifacts inside ./out/
    $cyan|help$reset|            Display this help message

$green|FLAGS:$reset|
    $cyan|-d$reset|               Enable debug mode (shows final instructions during build)

$green|DESCRIPTION:$reset|
    apex manages your dependencies, compiles your code, and handles project
    structure. It is designed to be the primary interface for Vertex developers.
"#
                );
            }
            "new" => {
                if let Some(project_name) = args.get(2) {
                    // TODO: Add appropriate directory
                    fs::create_dir(project_name)
                        .map_err(|_| CommandLineError::ErrorCreatingDirectory)?;

                    fs::create_dir(format!("{}/src", project_name))
                        .map_err(|_| CommandLineError::ErrorCreatingDirectory)?;

                    let mut config = File::create(format!("{}/prj.toml", project_name)).unwrap();

                    config
                        .write_all(format!("name = \"{}\"", project_name).as_bytes())
                        .unwrap();
                    let mut main_file = File::create(format!("{}/src/main.vtx", project_name))
                        .map_err(|_| CommandLineError::ErrorCreatingFile)?;

                    main_file
                        .write_all(b"writeLn!(\"Hello, world!\");\n")
                        .unwrap();
                }
            }
            "build" => {
                let debug = parse_flags(&args);

                let tex = fs::read_to_string("prj.toml")
                    .map_err(|_| CommandLineError::ErrorFindingDirectory)?;

                let config: Config = match toml::from_str(&tex) {
                    Err(e) => {
                        print!("{}", e);
                        process::exit(-1);
                    }
                    Ok(c) => c,
                };
                set_config(config.dependencies);

                File::open("src/main.vtx").unwrap_or_else(|_e| {
                    clrprintln!("$red|Linker error -> Cannot find main.vtx in ./src");
                    process::exit(-1);
                });

                rt.block_on(build_prj("src/".to_string(), config.name, debug))
            }
            "clear" => remove_dir_all("./out").unwrap(),
            _ => return Err(CommandLineError::NoSuchCommand),
        }
    } else {
        return Err(CommandLineError::InvalidCommand);
    }
    Ok(())
}

fn parse_flags(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-d")
}
