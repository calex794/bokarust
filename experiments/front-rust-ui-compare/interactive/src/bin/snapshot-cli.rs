use front_rust_interactive_probe::persistence;
use std::{env, fs, io, path::Path};
fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            "usage: snapshot-cli <seed|mutate|inspect|import|export|interrupt> <path> [source]"));
    }
    let path=Path::new(&args[2]);
    let proof=match args[1].as_str() {
        "seed" => persistence::persist(path, &persistence::seed(), false)?,
        "mutate" => {
            let existing=persistence::recover(path)?;
            persistence::persist(path, &persistence::mutate(&existing)?, false)?
        },
        "inspect" => persistence::receipt(&persistence::recover(path)?)?,
        "export" => {
            let bytes=persistence::recover(path)?;
            let output=args.get(3).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput,"missing export path"))?;
            fs::write(output,&bytes)?;
            persistence::receipt(&bytes)?
        },
        "import" => {
            let source=args.get(3).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput,"missing source path"))?;
            let bytes=fs::read(source)?;
            persistence::persist(path,&bytes,false)?
        },
        "interrupt" => {
            let existing=persistence::recover(path)?;
            persistence::persist(path,&persistence::mutate(&existing)?,true)?
        },
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput,"unknown command")),
    };
    println!("{}", proof.json());
    Ok(())
}