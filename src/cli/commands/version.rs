//! `tola version`: the Tola, Typst, and embedded Nu releases.

use anyhow::Result;
use serde::Serialize;

use crate::cli::args::VersionArgs;
use crate::cli::output::CommandOutput;

#[derive(Serialize)]
struct Versions {
    tola: &'static str,
    typst: &'static str,
    nu: &'static str,
}

pub(in crate::cli) fn run(args: &VersionArgs, output: &CommandOutput) -> Result<()> {
    let versions = Versions {
        tola: env!("CARGO_PKG_VERSION"),
        typst: typst::utils::version().raw(),
        nu: tola_nu::NU_VERSION,
    };
    if args.json {
        let encoded = if args.pretty {
            serde_json::to_string_pretty(&versions)?
        } else {
            serde_json::to_string(&versions)?
        };
        output.write_stdout_line(encoded)
    } else {
        output.write_stdout_line(format!(
            "tola {}\ntypst {}\nnu {}",
            versions.tola, versions.typst, versions.nu
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::Versions;

    #[test]
    fn json_has_exactly_three_versions() {
        let versions = Versions {
            tola: "0.0.0",
            typst: "0.0.0",
            nu: "0.0.0",
        };
        let value = serde_json::to_value(versions).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 3);
        assert!(object.contains_key("tola"));
        assert!(object.contains_key("typst"));
        assert!(object.contains_key("nu"));
    }
}
