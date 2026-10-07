//! The repo's `.herdr/services.toml`: named commands, nothing about placement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use toml_edit::{DocumentMut, Item};

pub const RESERVED: &str = "agent";
pub const SERVICES_PATH: &str = ".herdr/services.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub cmd: String,
    /// Absolute working directory (the repo root joined with `cwd`).
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub ready: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Services {
    pub file: Option<PathBuf>,
    pub map: BTreeMap<String, Service>,
}

impl Services {
    pub fn get(&self, name: &str) -> Option<&Service> {
        self.map.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    /// Find the services file for a directory: the nearest ancestor holding
    /// `.herdr/services.toml`. No file means no services.
    pub fn discover(dir: &Path) -> Result<Services> {
        for ancestor in dir.ancestors() {
            let candidate = ancestor.join(SERVICES_PATH);
            if candidate.is_file() {
                return Services::load(&candidate);
            }
        }
        Ok(Services::default())
    }

    pub fn load(path: &Path) -> Result<Services> {
        let text = std::fs::read_to_string(path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        let root = path
            .parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("/"));
        Services::parse(&text, path, root)
    }

    pub fn parse(text: &str, path: &Path, root: &Path) -> Result<Services> {
        let shown = path.display();
        let doc: DocumentMut = text
            .parse()
            .map_err(|e| anyhow!("{shown}: {}", crate::wsfile::describe_toml_error(text, &e)))?;
        let mut map = BTreeMap::new();
        for (name, item) in doc.iter() {
            if name == RESERVED {
                bail!("{shown}: service name `agent` is reserved for the workspace's own agent");
            }
            let table = item
                .as_table_like()
                .ok_or_else(|| anyhow!("{shown}: `{name}` must be a table like [{name}]"))?;
            let mut cmd = None;
            let mut cwd = root.to_path_buf();
            let mut env = BTreeMap::new();
            let mut ready = None;
            for (key, value) in table.iter() {
                match key {
                    "cmd" => cmd = Some(string(value, &shown, name, key)?),
                    "cwd" => cwd = root.join(string(value, &shown, name, key)?),
                    "ready" => ready = Some(string(value, &shown, name, key)?),
                    "env" => {
                        let env_table = value.as_table_like().ok_or_else(|| {
                            anyhow!("{shown}: service `{name}`: `env` must be a table of strings")
                        })?;
                        for (k, v) in env_table.iter() {
                            env.insert(k.to_string(), string(v, &shown, name, k)?);
                        }
                    }
                    other => bail!(
                        "{shown}: service `{name}`: unknown key `{other}` (allowed: cmd, cwd, env, ready)"
                    ),
                }
            }
            let cmd = cmd.ok_or_else(|| anyhow!("{shown}: service `{name}` has no `cmd`"))?;
            map.insert(
                name.to_string(),
                Service {
                    name: name.to_string(),
                    cmd,
                    cwd,
                    env,
                    ready,
                },
            );
        }
        Ok(Services {
            file: Some(path.to_path_buf()),
            map,
        })
    }
}

fn string(item: &Item, file: &impl std::fmt::Display, service: &str, key: &str) -> Result<String> {
    item.as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{file}: service `{service}`: `{key}` must be a string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Services> {
        Services::parse(text, Path::new("/r/.herdr/services.toml"), Path::new("/r"))
    }

    #[test]
    fn valid_file() {
        let s = parse(
            "[dev]\ncmd = \"pnpm dev\"\nready = \"ready in\"\n\n[test]\ncmd = \"pnpm test\"\ncwd = \"web\"\nenv = { CI = \"1\" }\n",
        )
        .unwrap();
        let dev = s.get("dev").unwrap();
        assert_eq!(dev.cmd, "pnpm dev");
        assert_eq!(dev.ready.as_deref(), Some("ready in"));
        assert_eq!(dev.cwd, PathBuf::from("/r"));
        let test = s.get("test").unwrap();
        assert_eq!(test.cwd, PathBuf::from("/r/web"));
        assert_eq!(test.env.get("CI").map(String::as_str), Some("1"));
    }

    #[test]
    fn missing_cmd() {
        let err = parse("[dev]\nready = \"x\"\n").unwrap_err().to_string();
        assert!(err.contains("`dev` has no `cmd`"), "{err}");
        assert!(err.contains("services.toml"), "{err}");
    }

    #[test]
    fn agent_is_reserved() {
        let err = parse("[agent]\ncmd = \"x\"\n").unwrap_err().to_string();
        assert!(err.contains("reserved"), "{err}");
    }

    #[test]
    fn placement_key_rejected() {
        let err = parse("[dev]\ncmd = \"x\"\ntab = \"main\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown key `tab`"), "{err}");
    }
}
