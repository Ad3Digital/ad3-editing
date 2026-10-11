//! Local font discovery with the established dapi flags and JSON Lines output.
use crate::{Result, encoder::cancelled, fonts::FontBook};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

pub const HELP: &str = "dapi-rust fonts [-f|--family <pattern>] [-w|--weight <weights...>] [-s|--style normal|italic] [-l|--limit <n>] [-n|--names-only]\n";

#[derive(Default)]
struct Options {
    family: Option<String>,
    weights: BTreeSet<String>,
    style: Option<String>,
    limit: Option<usize>,
    names: bool,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let (flag, inline) = args[index]
            .split_once('=')
            .map_or((args[index].as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        index += 1;
        if matches!(flag, "-n" | "--names-only") && inline.is_none() {
            options.names = true;
            continue;
        }
        if !matches!(
            flag,
            "-f" | "--family" | "-w" | "--weight" | "-s" | "--style" | "-l" | "--limit"
        ) {
            return Err(format!("Unknown fonts option: {flag}\n{HELP}"));
        }
        let value = if let Some(value) = inline {
            value
        } else {
            let value = args
                .get(index)
                .filter(|value| !value.starts_with('-'))
                .ok_or_else(|| format!("Missing value for {flag}"))?;
            index += 1;
            value.as_str()
        };
        match flag {
            "-f" | "--family" => options.family = Some(value.to_lowercase()),
            "-s" | "--style" => {
                if !matches!(value, "normal" | "italic") {
                    return Err("--style must be normal or italic".into());
                }
                options.style = Some(value.into());
            }
            "-l" | "--limit" => {
                options.limit = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|limit| *limit > 0)
                        .ok_or("--limit must be a positive integer")?,
                );
            }
            "-w" | "--weight" => {
                options.weights.insert(value.into());
                while let Some(weight) = args.get(index).filter(|value| !value.starts_with('-')) {
                    options.weights.insert(weight.clone());
                    index += 1;
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(options)
}

pub fn output(args: &[String], book: &mut FontBook, cancel: Option<&AtomicBool>) -> Result<String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
    {
        return Ok(HELP.into());
    }
    let options = parse(args)?;
    let mut output = String::new();
    let mut count = 0;
    for family in book.available_families(cancel)? {
        cancelled(cancel)?;
        if options
            .family
            .as_ref()
            .is_some_and(|pattern| !family.to_lowercase().contains(pattern))
        {
            continue;
        }
        // The family is a literal catalog name, not a CSS fallback list.
        let quoted = format!("\"{}\"", family.replace('\\', "\\\\").replace('"', "\\\""));
        let mut variants = book.available_variants(&quoted, cancel)?;
        variants.retain(|variant| {
            (options.weights.is_empty() || options.weights.contains(&variant.weight))
                && options
                    .style
                    .as_ref()
                    .is_none_or(|style| *style == variant.style)
        });
        if variants.is_empty() {
            continue;
        }
        if options.names {
            output.push_str(&family);
        } else {
            output
                .push_str(&serde_json::json!({"family": family, "variants": variants}).to_string());
        }
        output.push('\n');
        count += 1;
        if options.limit.is_some_and(|limit| count >= limit) {
            break;
        }
    }
    cancelled(cancel)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }
    #[test]
    fn font_command_filters_variants_formats_lines_and_validates_before_discovery() {
        let mut book = FontBook::default();
        let cancelled = AtomicBool::new(true);
        for invalid in [
            &["--limit", "0"][..],
            &["--limit", "1.5"],
            &["--style", "oblique"],
            &["--family"],
            &["--unknown"],
        ] {
            let error = output(&args(invalid), &mut book, Some(&cancelled)).unwrap_err();
            assert!(!error.contains("cancelled"), "{error}");
        }
        let output_json = output(
            &args(&[
                "--family=inter",
                "-w",
                "300",
                "700",
                "--style",
                "italic",
                "-l",
                "1",
            ]),
            &mut book,
            None,
        )
        .unwrap();
        assert_eq!(output_json.lines().count(), 1);
        let json: serde_json::Value = serde_json::from_str(output_json.trim()).unwrap();
        assert_eq!(json["family"], "Inter");
        let variants = json["variants"].as_array().unwrap();
        assert_eq!(variants.len(), 2);
        let inline_output = output(
            &args(&[
                "--family=inter",
                "--weight=300",
                "700",
                "--style=italic",
                "--limit=1",
            ]),
            &mut book,
            None,
        )
        .unwrap();
        assert_eq!(inline_output, output_json);
        for variant in variants {
            assert_eq!(variant["style"], "italic");
            assert!(matches!(variant["weight"].as_str(), Some("300" | "700")));
            assert!(variant["source"].as_str().unwrap().starts_with("local(\""));
        }
        assert_eq!(
            output(&args(&["-f", "inter", "-l", "1", "-n"]), &mut book, None).unwrap(),
            "Inter\n"
        );
        assert!(
            output(&args(&["-f", "MissingTestFamily"]), &mut book, None)
                .unwrap()
                .is_empty()
        );
        assert!(output(&args(&[]), &mut book, Some(&cancelled)).is_err());
    }
}
