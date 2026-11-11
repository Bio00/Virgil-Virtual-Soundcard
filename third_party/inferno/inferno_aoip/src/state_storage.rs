use std::{
  error::Error,
  fs::{create_dir_all, File},
  io::{Read, Write},
  path::MAIN_SEPARATOR_STR,
  sync::Arc,
};

use crate::{common::*, device_info, device_info::DeviceInfo};
use platform_dirs::AppDirs;
use serde::{Deserialize, Serialize};
use toml;

const PATH_SUFFIX: &str = ".toml";

pub struct StateStorage {
  path_prefix: String,
}

impl StateStorage {
  pub fn new(self_info: &DeviceInfo) -> Self {
    // Virgil patch: services may run as a user without a home directory
    // (systemd DynamicUser); fall back to the temp directory instead of
    // panicking, which stopped the whole device from starting.
    let base = AppDirs::new(Some("inferno_aoip"), false)
      .map(|d| d.state_dir)
      .unwrap_or_else(|| std::env::temp_dir().join("inferno_aoip"));
    let dir_for = |id: &[u8]| base.to_string_lossy().into_owned() + MAIN_SEPARATOR_STR + &hex::encode(id);
    let dir = dir_for(&self_info.factory_device_id);
    // Virgil patch: carry state saved under an older device id over to the
    // current one. Ids used to be built from the IP address, then from the MAC
    // address with process id 0 (which Dante software on the same computer
    // uses for itself); newest first.
    let mut legacy_ids = Vec::new();
    let mut mac_zero = self_info.factory_device_id;
    mac_zero[6..8].copy_from_slice(&[0, 0]);
    legacy_ids.push(mac_zero);
    for pid in [self_info.process_id, 0] {
      let mut ip_based = [0u8; 8];
      ip_based[2..6].copy_from_slice(&self_info.ip_address.octets());
      ip_based[6..8].copy_from_slice(&pid.to_be_bytes());
      legacy_ids.push(ip_based);
    }
    let legacy_dirs: Vec<String> = legacy_ids.iter().map(|id| dir_for(id)).collect();
    migrate_legacy_state(&dir, &legacy_dirs);
    create_dir_all(&dir).log_and_forget();
    info!("using state directory: {dir}");
    Self { path_prefix: dir + MAIN_SEPARATOR_STR }
  }
  fn full_path(&self, name: &str) -> String {
    format!("{}{name}{PATH_SUFFIX}", self.path_prefix)
  }
  pub fn save(&self, name: &str, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let content = toml::to_string(&value)?;
    let tmp_path = self.full_path(&format!("tmp.{name}"));
    let mut file = File::create(&tmp_path)?;
    file.write(content.as_bytes())?;
    drop(file);
    std::fs::rename(tmp_path, self.full_path(name))?;
    Ok(())
  }
  pub fn load<T: for<'a> Deserialize<'a>>(&self, name: &str) -> Result<T, Box<dyn Error>> {
    let mut file = File::open(self.full_path(name))?;
    let mut content: String = "".to_owned();
    file.read_to_string(&mut content)?;
    Ok(toml::from_str(&content)?)
  }
}

/// Virgil patch: when `dir` does not exist yet, rename the first of
/// `legacy_dirs` that does into it. Returns the one moved, if any.
fn migrate_legacy_state(dir: &str, legacy_dirs: &[String]) -> Option<String> {
  if std::path::Path::new(dir).exists() {
    return None;
  }
  for legacy_dir in legacy_dirs {
    if legacy_dir == dir || !std::path::Path::new(legacy_dir).is_dir() {
      continue;
    }
    return match std::fs::rename(legacy_dir, dir) {
      Ok(()) => {
        info!("moved saved state from {legacy_dir}");
        Some(legacy_dir.clone())
      }
      Err(e) => {
        warn!("could not move saved state from {legacy_dir}: {e}");
        None
      }
    };
  }
  None
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::{SystemTime, UNIX_EPOCH};

  fn temp_state_storage() -> StateStorage {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("inferno_aoip_test_{ts}")).to_str().unwrap().to_owned();
    std::fs::create_dir_all(&dir).unwrap();
    StateStorage { path_prefix: dir + MAIN_SEPARATOR_STR }
  }

  #[derive(Serialize, Deserialize, Debug, PartialEq)]
  struct TestConfig {
    name: String,
    count: u32,
  }

  fn temp_base(tag: &str) -> std::path::PathBuf {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let base = std::env::temp_dir().join(format!("inferno_aoip_migrate_{tag}_{ts}"));
    std::fs::create_dir_all(&base).unwrap();
    base
  }

  #[test]
  fn migrates_first_existing_legacy_dir() {
    let base = temp_base("first");
    let p = |n: &str| base.join(n).to_string_lossy().into_owned();
    std::fs::create_dir_all(p("mac0")).unwrap();
    std::fs::write(base.join("mac0").join("rx.toml"), "x = 1").unwrap();
    std::fs::create_dir_all(p("ip")).unwrap();
    let moved = migrate_legacy_state(&p("new"), &[p("missing"), p("mac0"), p("ip")]);
    assert_eq!(moved, Some(p("mac0")));
    assert!(base.join("new").join("rx.toml").exists());
    assert!(!base.join("mac0").exists());
    assert!(base.join("ip").exists());
  }

  #[test]
  fn does_not_migrate_over_existing_state() {
    let base = temp_base("existing");
    let p = |n: &str| base.join(n).to_string_lossy().into_owned();
    std::fs::create_dir_all(p("new")).unwrap();
    std::fs::create_dir_all(p("mac0")).unwrap();
    assert_eq!(migrate_legacy_state(&p("new"), &[p("mac0")]), None);
    assert!(base.join("mac0").exists());
  }

  #[test]
  fn nothing_to_migrate() {
    let base = temp_base("none");
    let p = |n: &str| base.join(n).to_string_lossy().into_owned();
    assert_eq!(migrate_legacy_state(&p("new"), &[p("new"), p("missing")]), None);
    assert!(!base.join("new").exists());
  }

  #[test]
  fn save_and_load_roundtrip() {
    let storage = temp_state_storage();
    let config = TestConfig { name: "test".to_string(), count: 42 };
    storage.save("config", &config).unwrap();
    let loaded: TestConfig = storage.load("config").unwrap();
    assert_eq!(config, loaded);
  }

  #[test]
  fn load_missing_file_fails() {
    let storage = temp_state_storage();
    let result: Result<TestConfig, Box<dyn Error>> = storage.load("nonexistent");
    assert!(result.is_err());
  }

  #[test]
  fn save_overwrites_existing() {
    let storage = temp_state_storage();
    let config1 = TestConfig { name: "first".to_string(), count: 1 };
    let config2 = TestConfig { name: "second".to_string(), count: 2 };
    storage.save("config", &config1).unwrap();
    storage.save("config", &config2).unwrap();
    let loaded: TestConfig = storage.load("config").unwrap();
    assert_eq!(loaded.name, "second");
    assert_eq!(loaded.count, 2);
  }

  #[test]
  fn save_uses_atomic_rename() {
    let storage = temp_state_storage();
    let config = TestConfig { name: "atomic".to_string(), count: 99 };
    storage.save("atomic", &config).unwrap();
    // tmp file should not exist after rename
    let tmp_path = format!("{}tmp.atomic{PATH_SUFFIX}", storage.path_prefix);
    assert!(!std::path::Path::new(&tmp_path).exists());
    // final file should exist
    let final_path = format!("{}atomic{PATH_SUFFIX}", storage.path_prefix);
    assert!(std::path::Path::new(&final_path).exists());
  }

  #[test]
  fn load_malformed_toml_fails() {
    let storage = temp_state_storage();
    let path = format!("{}bad{PATH_SUFFIX}", storage.path_prefix);
    std::fs::write(&path, "not valid toml [[[").unwrap();
    let result: Result<TestConfig, Box<dyn Error>> = storage.load("bad");
    assert!(result.is_err());
  }
}
