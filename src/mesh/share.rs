//! 소유 기기 쪽: 공유 이름 → 이 기기의 폴더 (설계 7.4의 2)
//!
//! 접속 기기는 `\\서버\공유\아래\경로`의 공유 이름과 아래 경로를 보낸다. 소유 기기는 자기 공유 설정으로 공유 이름을
//! 자기 폴더로 바꾼다.
//! - Linux(Samba): `/etc/samba/smb.conf`의 공유별 `path`(`include`로 이은 파일 포함), `net usershare`로 만든 공유
//!   (`/var/lib/samba/usershares`), `[homes]`(사용자 이름 = 공유 이름).
//! - Windows: 서버 서비스의 공유 목록(레지스트리, 관리자 권한 없이 읽힌다) (`platform::share_folder`).
//!
//! 공유하지 않은 폴더는 받지 않는다(설계 7.4 "받는 폴더의 범위"): 아래 경로를 붙인 뒤에도 실제 위치(링크를 푼 것)가
//! 그 공유 폴더 아래여야 한다(`folder_under`).

use std::path::{Path, PathBuf};

/// smb.conf를 읽은 것 (Samba를 쓰는 Linux에서만 쓴다)
#[cfg_attr(windows, allow(dead_code))]
#[derive(Debug, Default, PartialEq)]
pub struct SmbConf {
    /// (공유 이름, 폴더). `[global]`·`[printers]`·`[homes]`는 빼고, `available = no`인 공유도 뺀다.
    pub shares: Vec<(String, String)>,
    /// `include =`로 이은 파일
    pub includes: Vec<String>,
    /// `[homes]`가 있으면 그 `path` (비어 있으면 사용자 홈 폴더)
    pub homes: Option<String>,
}

/// smb.conf 글을 읽는다. 키 이름은 대소문자와 공백을 가리지 않고, `#`·`;`로 시작하는 줄은 주석, `\`로 끝나면 다음 줄에 이어진다.
#[cfg_attr(windows, allow(dead_code))]
pub fn parse_smb_conf(text: &str) -> SmbConf {
    let mut out = SmbConf::default();
    // (이름, path, 쓸 수 있는지)
    let mut cur: Option<(String, String, bool)> = None;
    let finish = |cur: &mut Option<(String, String, bool)>, out: &mut SmbConf| {
        if let Some((name, path, available)) = cur.take() {
            let lower = name.to_lowercase();
            if lower == "homes" {
                out.homes = Some(path);
            } else if available && !path.is_empty() && !matches!(lower.as_str(), "global" | "printers" | "print$") {
                out.shares.push((name, path));
            }
        }
    };
    let mut logical = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if logical.is_empty() && (line.starts_with('#') || line.starts_with(';')) {
            continue;
        }
        if let Some(head) = line.strip_suffix('\\') {
            logical.push_str(head);
            continue;
        }
        logical.push_str(line);
        let line = std::mem::take(&mut logical);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.split_once(']')).map(|(n, _)| n.trim()) {
            finish(&mut cur, &mut out);
            cur = Some((name.to_string(), String::new(), true));
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let key: String = k.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        let value = v.trim().trim_matches('"').to_string();
        match key.as_str() {
            "include" => out.includes.push(value),
            "path" | "directory" => {
                if let Some(c) = cur.as_mut() {
                    c.1 = value;
                }
            }
            "available" => {
                if let Some(c) = cur.as_mut() {
                    c.2 = !matches!(value.to_lowercase().as_str(), "no" | "false" | "0");
                }
            }
            _ => {}
        }
    }
    finish(&mut cur, &mut out);
    out
}

/// 공유 폴더 경로의 대체 문자: `%S`(공유 이름), `%U`·`%u`(사용자 = 공유 이름으로 들어오는 [homes])
#[cfg_attr(windows, allow(dead_code))]
fn substitute(path: &str, share: &str) -> String {
    path.replace("%S", share).replace("%U", share).replace("%u", share)
}

/// 공유 이름 → 이 기기의 폴더. 찾지 못하면 그 까닭.
pub fn resolve(name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\', '\0']) {
        return Err("공유 이름이 올바르지 않습니다".to_string());
    }
    #[cfg(windows)]
    {
        crate::platform::share_folder(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("이 기기의 공유 목록에 '{name}'이 없습니다"))
    }
    #[cfg(unix)]
    {
        samba_share(name)
    }
}

/// Samba 설정 파일 (시험용으로 환경 변수 `GEMMA_SEARCH_SMB_CONF`로 바꿀 수 있다)
#[cfg(unix)]
fn smb_conf_path() -> PathBuf {
    std::env::var_os("GEMMA_SEARCH_SMB_CONF")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/samba/smb.conf"))
}

#[cfg(unix)]
fn samba_share(name: &str) -> Result<PathBuf, String> {
    let main = smb_conf_path();
    let Ok(text) = std::fs::read_to_string(&main) else {
        return usershare(name).ok_or_else(|| {
            format!("Samba 설정({})을 읽지 못했고 '{name}' 공유도 찾지 못했습니다", main.display())
        });
    };
    // include로 이은 파일까지 (깊이 4까지, 변수가 든 이름은 건너뛴다)
    let mut conf = parse_smb_conf(&text);
    let mut todo: Vec<(String, usize)> = conf.includes.drain(..).map(|i| (i, 1)).collect();
    while let Some((file, depth)) = todo.pop() {
        if depth > 4 || file.contains('%') {
            continue;
        }
        if let Ok(t) = std::fs::read_to_string(&file) {
            let mut c = parse_smb_conf(&t);
            todo.extend(c.includes.drain(..).map(|i| (i, depth + 1)));
            conf.shares.extend(c.shares);
            if conf.homes.is_none() {
                conf.homes = c.homes;
            }
        }
    }
    if let Some((n, path)) = conf.shares.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Ok(PathBuf::from(substitute(path, n)));
    }
    if let Some(p) = usershare(name) {
        return Ok(p);
    }
    if let Some(homes) = &conf.homes {
        let path = if homes.trim().is_empty() { home_of(name) } else { Some(substitute(homes, name)) };
        if let Some(p) = path.map(PathBuf::from).filter(|p| p.is_dir()) {
            return Ok(p);
        }
    }
    Err(format!("Samba 설정({})에 '{name}' 공유가 없습니다", main.display()))
}

/// `net usershare`로 만든 공유: `/var/lib/samba/usershares/<소문자 이름>`의 `path=`
#[cfg(unix)]
fn usershare(name: &str) -> Option<PathBuf> {
    let text = std::fs::read_to_string(Path::new("/var/lib/samba/usershares").join(name.to_lowercase())).ok()?;
    text.lines()
        .find_map(|l| l.trim().strip_prefix("path="))
        .map(|p| PathBuf::from(p.trim()))
}

/// 사용자 홈 폴더 (`/etc/passwd`)
#[cfg(unix)]
fn home_of(user: &str) -> Option<String> {
    let text = std::fs::read_to_string("/etc/passwd").ok()?;
    text.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 6 && f[0] == user).then(|| f[5].to_string())
    })
}

/// `\\?\C:\…` → `C:\…`, `\\?\UNC\서버\…` → `\\서버\…` (Windows의 canonicalize가 붙이는 머리)
pub fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p
}

/// 공유 폴더(`base`) 아래 이름들(`sub`)을 붙인 폴더. `..`이나 구분자가 든 이름은 받지 않고, 링크를 풀었을 때
/// 공유 폴더 밖으로 나가면 거절한다. 폴더가 없어도 거절한다.
pub fn folder_under(base: &Path, sub: &[String]) -> Result<PathBuf, String> {
    for s in sub {
        if s.is_empty() || s == ".." || s == "." || s.contains(['/', '\\', '\0']) || (cfg!(windows) && s.contains(':')) {
            return Err(format!("경로의 이름이 올바르지 않습니다: {s}"));
        }
    }
    let joined = sub.iter().fold(base.to_path_buf(), |p, s| p.join(s));
    let real_base = std::fs::canonicalize(base).map_err(|e| format!("공유 폴더({})를 열 수 없습니다: {e}", base.display()))?;
    let real = std::fs::canonicalize(&joined).map_err(|_| format!("이 기기에 그 폴더가 없습니다: {}", joined.display()))?;
    if !real.starts_with(&real_base) {
        return Err("공유한 폴더 밖을 가리킵니다 (링크)".to_string());
    }
    if !real.is_dir() {
        return Err(format!("폴더가 아닙니다: {}", joined.display()));
    }
    Ok(plain(real))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_samba_shares() {
        let conf = r#"
[global]
   workgroup = WORKGROUP
   include = /etc/samba/extra.conf
# [hidden]
;  path = /nope

[공유]
   comment = 집 NAS
   path = /mnt/storage/공유
   read only = no

[Media]
   Path = "/mnt/storage/Media"
[off]
   path = /mnt/off
   available = no
[long]
   path = /mnt/very/\
long
[homes]
   browseable = no
[printers]
   path = /var/spool/samba
"#;
        let c = parse_smb_conf(conf);
        assert_eq!(
            c.shares,
            vec![
                ("공유".to_string(), "/mnt/storage/공유".to_string()),
                ("Media".to_string(), "/mnt/storage/Media".to_string()),
                ("long".to_string(), "/mnt/very/long".to_string()),
            ]
        );
        assert_eq!(c.includes, vec!["/etc/samba/extra.conf"]);
        assert_eq!(c.homes, Some(String::new()));
        assert_eq!(substitute("/srv/%S", "media"), "/srv/media");
    }

    #[test]
    fn folders_stay_inside_the_share() {
        let base = std::env::temp_dir().join(format!("gs-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("사진").join("2024")).unwrap();
        std::fs::write(base.join("a.txt"), b"x").unwrap();
        let ok = folder_under(&base, &["사진".into(), "2024".into()]).unwrap();
        assert!(ok.ends_with(Path::new("사진").join("2024")));
        assert!(!ok.to_string_lossy().starts_with(r"\\?\"));
        assert!(folder_under(&base, &[]).is_ok());
        assert!(folder_under(&base, &["..".into()]).is_err());
        assert!(folder_under(&base, &["사진/../..".into()]).is_err());
        assert!(folder_under(&base, &["없음".into()]).is_err());
        assert!(folder_under(&base, &["a.txt".into()]).is_err());
        #[cfg(unix)]
        {
            // 공유 밖을 가리키는 링크
            std::os::unix::fs::symlink("/tmp", base.join("밖")).unwrap();
            assert!(folder_under(&base, &["밖".into()]).is_err());
        }
        assert_eq!(plain(PathBuf::from(r"\\?\UNC\nas\x")), PathBuf::from(r"\\nas\x"));
        assert_eq!(plain(PathBuf::from(r"\\?\D:\x")), PathBuf::from(r"D:\x"));
        let _ = std::fs::remove_dir_all(base);
    }
}
