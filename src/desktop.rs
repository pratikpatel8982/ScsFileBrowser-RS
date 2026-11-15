//! Freedesktop `.desktop` application entries (Linux/BSD): finding the installed applications
//! and building the command line that opens a file with one of them.
//!
//! The parsing is plain text handling with no OS calls, so it is available (and tested) on every
//! platform; only [`installed_apps`] looks at the real application directories.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// An application described by a `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// The `.desktop` file this was read from.
    pub path: PathBuf,
    pub name: String,
    pub comment: String,
    pub exec: String,
    pub try_exec: Option<String>,
    pub working_dir: Option<String>,
    pub terminal: bool,
    /// `NoDisplay` or `Hidden`: valid, but not meant to be listed in menus.
    pub hidden: bool,
    /// `Type=Application`.
    pub is_application: bool,
}

/// True for a path that names a `.desktop` file.
pub fn is_desktop_file(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("desktop"))
}

/// Undo the string escapes a `.desktop` value may contain (`\s \n \t \r \\`).
fn unescape_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

impl DesktopEntry {
    /// Parse the `[Desktop Entry]` group of a `.desktop` file's text.
    pub fn parse(path: &Path, text: &str) -> Option<DesktopEntry> {
        let mut in_group = false;
        let mut seen_group = false;
        let mut e = DesktopEntry {
            path: path.to_path_buf(),
            name: String::new(),
            comment: String::new(),
            exec: String::new(),
            try_exec: None,
            working_dir: None,
            terminal: false,
            hidden: false,
            is_application: false,
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                in_group = line == "[Desktop Entry]";
                seen_group |= in_group;
                continue;
            }
            if !in_group {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            // Localised keys (`Name[de]`) and unknown keys are ignored.
            let value = value.trim();
            match key.trim() {
                "Type" => e.is_application = value == "Application",
                "Name" => e.name = unescape_value(value),
                "Comment" => e.comment = unescape_value(value),
                "Exec" => e.exec = unescape_value(value),
                "TryExec" => e.try_exec = Some(unescape_value(value)),
                "Path" => e.working_dir = Some(unescape_value(value)),
                "Terminal" => e.terminal = value == "true",
                "NoDisplay" | "Hidden" if value == "true" => e.hidden = true,
                _ => {}
            }
        }
        if !seen_group {
            return None;
        }
        if e.name.is_empty() {
            e.name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        Some(e)
    }

    pub fn load(path: &Path) -> io::Result<DesktopEntry> {
        let text = fs::read_to_string(path)?;
        DesktopEntry::parse(path, &text).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("'{}' is not a desktop entry", path.display()),
            )
        })
    }

    /// The `Exec` line with the arguments split and the field codes filled in for `file`.
    pub fn command_line(&self, file: &Path) -> Result<Vec<String>, String> {
        if !self.is_application {
            return Err("it is not an application entry".into());
        }
        if self.terminal {
            return Err("applications that need a terminal are not supported".into());
        }
        let args = split_exec(&self.exec)?;
        let mut out = Vec::new();
        let mut used_file = false;
        for arg in &args {
            // A lone %F / %U stands for the whole file list.
            if arg == "%F" || arg == "%U" {
                out.push(if arg == "%U" {
                    file_uri(file)
                } else {
                    file.to_string_lossy().into_owned()
                });
                used_file = true;
                continue;
            }
            let (expanded, used) = expand_field_codes(arg, file, self);
            used_file |= used;
            // An argument that was only a removed code (`%i`) disappears.
            if !(expanded.is_empty() && arg.starts_with('%')) {
                out.push(expanded);
            }
        }
        if out.is_empty() {
            return Err("it has no Exec command".into());
        }
        if !used_file {
            // Entries without a file code (`Exec=kate`) normally can't be given files, but the
            // whole point of "open with" is to pass one, and nearly all of them accept it.
            out.push(file.to_string_lossy().into_owned());
        }
        Ok(out)
    }

    /// Start the application with `file`. The caller reaps the child.
    pub fn launch(&self, file: &Path) -> io::Result<Child> {
        let argv = self
            .command_line(file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        if let Some(dir) = self
            .working_dir
            .as_deref()
            .filter(|d| Path::new(d).is_dir())
        {
            cmd.current_dir(dir);
        }
        cmd.spawn()
    }

    /// Whether the application is worth showing in a picker.
    fn listable(&self) -> bool {
        if !self.is_application || self.hidden || self.terminal || self.exec.trim().is_empty() {
            return false;
        }
        match &self.try_exec {
            Some(t) if !t.is_empty() => find_in_path(t),
            _ => true,
        }
    }
}

/// Split an `Exec` value into arguments (Desktop Entry spec, "The Exec key").
fn split_exec(exec: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut in_quote = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quote {
            match c {
                '"' => in_quote = false,
                '\\' => match chars.peek() {
                    Some(&n) if matches!(n, '"' | '`' | '$' | '\\') => {
                        cur.push(n);
                        chars.next();
                    }
                    _ => cur.push('\\'),
                },
                _ => cur.push(c),
            }
        } else {
            match c {
                '"' => {
                    in_quote = true;
                    started = true;
                }
                ' ' | '\t' => {
                    if started {
                        args.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                _ => {
                    cur.push(c);
                    started = true;
                }
            }
        }
    }
    if in_quote {
        return Err("the Exec line has an unterminated quote".into());
    }
    if started {
        args.push(cur);
    }
    Ok(args)
}

/// Replace the field codes inside one argument. Returns the text and whether a file code was used.
fn expand_field_codes(arg: &str, file: &Path, entry: &DesktopEntry) -> (String, bool) {
    let mut out = String::new();
    let mut used = false;
    let mut chars = arg.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('f' | 'F') => {
                out.push_str(&file.to_string_lossy());
                used = true;
            }
            Some('u' | 'U') => {
                out.push_str(&file_uri(file));
                used = true;
            }
            Some('c') => out.push_str(&entry.name),
            Some('k') => out.push_str(&entry.path.to_string_lossy()),
            // %i (icon) and the deprecated codes expand to nothing.
            Some(_) | None => {}
        }
    }
    (out, used)
}

/// `file://` URI for a path, percent-encoding everything but unreserved characters and `/`.
fn file_uri(path: &Path) -> String {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = String::from("file://");
    for &b in abs.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn find_in_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).exists();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).exists()))
}

/// The `applications` directories to search, most important first.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut data_dirs: Vec<PathBuf> = Vec::new();
    let home_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
    if let Some(d) = home_data.clone() {
        data_dirs.push(d);
    }
    match std::env::var("XDG_DATA_DIRS") {
        Ok(v) if !v.is_empty() => data_dirs.extend(v.split(':').map(PathBuf::from)),
        _ => {
            data_dirs.push("/usr/local/share".into());
            data_dirs.push("/usr/share".into());
        }
    }
    // Flatpak and Snap export their entries outside the standard variables on many setups.
    if let Some(d) = home_data {
        data_dirs.push(d.join("flatpak/exports/share"));
    }
    data_dirs.push("/var/lib/flatpak/exports/share".into());
    data_dirs.push("/var/lib/snapd/desktop".into());

    let mut seen = HashSet::new();
    data_dirs
        .into_iter()
        .filter(|d| d.is_absolute() && seen.insert(d.clone()))
        .map(|d| d.join("applications"))
        .collect()
}

/// Every listable application found in `dirs`, sorted by name. A `.desktop` file id that appears
/// in several directories is taken from the first one.
pub fn installed_apps_in(dirs: &[PathBuf]) -> Vec<DesktopEntry> {
    fn walk(dir: &Path, prefix: &str, depth: u32, out: &mut Vec<(String, PathBuf)>) {
        let Ok(rd) = fs::read_dir(dir) else {
            return;
        };
        for item in rd.flatten() {
            let path = item.path();
            let file_name = item.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if depth < 4 {
                    walk(&path, &format!("{prefix}{file_name}-"), depth + 1, out);
                }
            } else if is_desktop_file(&file_name) {
                out.push((format!("{prefix}{file_name}"), path));
            }
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let mut found = Vec::new();
        walk(dir, "", 0, &mut found);
        found.sort();
        for (id, path) in found {
            if !seen.insert(id) {
                continue;
            }
            match DesktopEntry::load(&path) {
                Ok(e) if e.listable() => apps.push(e),
                Ok(_) => {}
                Err(e) => log::debug!("Skipping '{}': {e}", path.display()),
            }
        }
    }
    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.path.clone()));
    apps
}

/// Every application installed on this system.
pub fn installed_apps() -> Vec<DesktopEntry> {
    installed_apps_in(&application_dirs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(exec: &str) -> DesktopEntry {
        let text = format!("[Desktop Entry]\nType=Application\nName=Test App\nExec={exec}\n");
        DesktopEntry::parse(Path::new("/usr/share/applications/test.desktop"), &text).unwrap()
    }

    #[test]
    fn parses_the_main_group_only() {
        let text = "# comment\n[Desktop Entry]\nType=Application\nName=Kate\nName[de]=Kätchen\n\
                    Comment=Edit text\nExec=kate -b %U\nTerminal=false\n\
                    [Desktop Action new]\nName=New\nExec=kate --new\n";
        let e = DesktopEntry::parse(Path::new("/x/org.kde.kate.desktop"), text).unwrap();
        assert_eq!(e.name, "Kate");
        assert_eq!(e.comment, "Edit text");
        assert_eq!(e.exec, "kate -b %U");
        assert!(e.is_application && !e.hidden && !e.terminal);
        assert!(DesktopEntry::parse(Path::new("/x/a.desktop"), "just text").is_none());
    }

    #[test]
    fn name_falls_back_to_the_file_stem() {
        let e = DesktopEntry::parse(
            Path::new("/x/org.kde.kate.desktop"),
            "[Desktop Entry]\nType=Application\nExec=kate\n",
        )
        .unwrap();
        assert_eq!(e.name, "org.kde.kate");
    }

    // `file://` URIs of POSIX paths: a Windows path would be rewritten.
    #[cfg(unix)]
    #[test]
    fn field_codes_are_substituted() {
        let file = Path::new("/tmp/my file.sii");
        assert_eq!(
            entry("kate -b %F").command_line(file).unwrap(),
            ["kate", "-b", "/tmp/my file.sii"]
        );
        assert_eq!(
            entry("viewer %U").command_line(file).unwrap(),
            ["viewer", "file:///tmp/my%20file.sii"]
        );
        assert_eq!(
            entry("app --file=%f --icon %i 100%% %c")
                .command_line(file)
                .unwrap(),
            [
                "app",
                "--file=/tmp/my file.sii",
                "--icon",
                "100%",
                "Test App"
            ]
        );
    }

    #[test]
    fn file_is_appended_when_there_is_no_file_code() {
        let file = Path::new("/tmp/a.sii");
        assert_eq!(
            entry("kate").command_line(file).unwrap(),
            ["kate", "/tmp/a.sii"]
        );
    }

    #[test]
    fn quoting_follows_the_spec() {
        let file = Path::new("/tmp/a.sii");
        assert_eq!(
            entry(r#""/opt/My App/run" --name "a \"b\" \$c" %f"#)
                .command_line(file)
                .unwrap(),
            ["/opt/My App/run", "--name", r#"a "b" $c"#, "/tmp/a.sii"]
        );
        assert!(entry(r#"app "unterminated"#).command_line(file).is_err());
    }

    // `file://` URIs of POSIX paths: a Windows path would be rewritten.
    #[cfg(unix)]
    #[test]
    fn flatpak_style_exec() {
        let file = Path::new("/tmp/a.sii");
        assert_eq!(
            entry("/usr/bin/flatpak run --branch=stable --command=kate org.kde.kate @@u %U @@")
                .command_line(file)
                .unwrap(),
            [
                "/usr/bin/flatpak",
                "run",
                "--branch=stable",
                "--command=kate",
                "org.kde.kate",
                "@@u",
                "file:///tmp/a.sii",
                "@@"
            ]
        );
    }

    #[test]
    fn unsuitable_entries_are_refused() {
        let file = Path::new("/tmp/a.sii");
        let term = DesktopEntry::parse(
            Path::new("/x/vim.desktop"),
            "[Desktop Entry]\nType=Application\nName=Vim\nExec=vim %F\nTerminal=true\n",
        )
        .unwrap();
        assert!(term.command_line(file).is_err());
        assert!(!term.listable());
        let link = DesktopEntry::parse(
            Path::new("/x/l.desktop"),
            "[Desktop Entry]\nType=Link\nName=L\nURL=http://x\n",
        )
        .unwrap();
        assert!(link.command_line(file).is_err());
    }

    #[test]
    fn detects_desktop_paths() {
        assert!(is_desktop_file(
            "/usr/share/applications/org.kde.kate.desktop"
        ));
        assert!(is_desktop_file("A.DESKTOP"));
        assert!(!is_desktop_file("/usr/bin/kate"));
    }

    #[test]
    fn discovery_dedups_filters_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(&b).unwrap();
        let w = |p: PathBuf, body: &str| fs::write(p, body).unwrap();
        let app = |name: &str, extra: &str| {
            format!("[Desktop Entry]\nType=Application\nName={name}\nExec=x %f\n{extra}")
        };
        w(a.join("zed.desktop"), &app("Zed", ""));
        w(a.join("hidden.desktop"), &app("Hidden", "NoDisplay=true\n"));
        w(
            a.join("gone.desktop"),
            &app("Gone", "TryExec=/definitely/not/here\n"),
        );
        w(a.join("sub/inner.desktop"), &app("inner", ""));
        w(a.join("readme.txt"), "not an entry");
        // Same id in a later directory: the first one wins.
        w(b.join("zed.desktop"), &app("Zed (system copy)", ""));
        w(b.join("Alpha.desktop"), &app("alpha", ""));

        let names: Vec<String> = installed_apps_in(&[a, b])
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["alpha", "inner", "Zed"]);
    }

    #[cfg(unix)]
    #[test]
    fn launches_through_the_exec_line() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out.txt");
        let file = tmp.path().join("in file.sii");
        let desk = tmp.path().join("t.desktop");
        fs::write(
            &desk,
            format!(
                "[Desktop Entry]\nType=Application\nName=T\nPath={}\nExec=sh -c \"echo $0 > {}; pwd >> {}\" %f\n",
                tmp.path().display(),
                out.display(),
                out.display()
            ),
        )
        .unwrap();
        let mut child = DesktopEntry::load(&desk).unwrap().launch(&file).unwrap();
        child.wait().unwrap();
        let text = fs::read_to_string(&out).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), file.to_string_lossy());
        assert_eq!(
            fs::canonicalize(lines.next().unwrap()).unwrap(),
            fs::canonicalize(tmp.path()).unwrap()
        );
    }
}
