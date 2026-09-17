// Метка сборки для `--version`, `serverInfo` и стартовой записи журнала.
//
// Берётся коммит, а не время запуска `cargo`: он однозначно указывает на
// исходники сборки и одинаков при повторной сборке тех же исходников. Правки
// поверх коммита метку не подделывают — они видны суффиксом `+dirty`.

use std::path::{Path, PathBuf};
use std::process::Command;

// Метка, когда git недоступен (сборка из архива исходников).
const UNKNOWN: &str = "unknown";

fn main() {
    watch_git_head();

    // Метка, заданная снаружи, побеждает: при сборке в контейнере рабочее дерево
    // выглядит грязным из-за переводов строк (индекс LF, checkout на Windows
    // CRLF), и git внутри контейнера пометил бы чистую сборку как +dirty.
    if let (Ok(commit), Ok(date)) = (
        std::env::var("BUILD_COMMIT_OVERRIDE"),
        std::env::var("BUILD_DATE_OVERRIDE"),
    ) {
        println!("cargo:rustc-env=BUILD_COMMIT={}", commit);
        println!("cargo:rustc-env=BUILD_DATE={}", date);
        return;
    }

    let (commit, date) = match git(&["rev-parse", "--short", "HEAD"]) {
        Some(commit) => {
            let dirty = if git_failed(&["diff", "--quiet", "HEAD"]) {
                "+dirty"
            } else {
                ""
            };
            let date = git(&["log", "-1", "--format=%cd", "--date=short"])
                .unwrap_or_else(|| UNKNOWN.to_string());

            (format!("{}{}", commit, dirty), date)
        }
        None => (UNKNOWN.to_string(), UNKNOWN.to_string()),
    };

    println!("cargo:rustc-env=BUILD_COMMIT={}", commit);
    println!("cargo:rustc-env=BUILD_DATE={}", date);
}

// Пересборка при смене коммита или ветки. Без этого метка застынет на первой
// сборке: новый коммит не меняет ни одного файла, за которым cargo следит сам.
fn watch_git_head() {
    let Some(git_dir) = [Path::new("../.git"), Path::new(".git")]
        .into_iter()
        .find(|p| p.exists())
    else {
        return;
    };

    let head = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());

    // Коммит в текущую ветку меняет не HEAD, а файл ссылки, на которую он смотрит.
    if let Ok(content) = std::fs::read_to_string(&head) {
        if let Some(reference) = content.strip_prefix("ref: ") {
            let ref_path: PathBuf = git_dir.join(reference.trim());
            if ref_path.exists() {
                println!("cargo:rerun-if-changed={}", ref_path.display());
            }
        }
    }
}

// Вывод git-команды одной строкой; None — git недоступен или отказал.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

// Отказ git-команды. Недоступный git отказом не считается: метка тогда уже
// unknown, помечать её грязной нечем.
fn git_failed(args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .output()
        .map(|out| !out.status.success())
        .unwrap_or(false)
}
