//! Windows 로그인 시 실행: HKCU\Software\Microsoft\Windows\CurrentVersion\Run 에 "exe 경로" --background
//!
//! 포터블이라 exe를 옮길 수 있으므로, 켜져 있으면 실행할 때마다 경로를 다시 맞춘다.

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteKeyValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "GemmaSearch";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// 등록할 명령줄
pub fn command_line() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --background", exe.display()))
}

/// 지금 등록된 값
pub fn current() -> Option<String> {
    let key = wide(RUN_KEY);
    let name = wide(VALUE);
    let mut buf = vec![0u16; 2048];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut len,
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// 켜기(지금 exe 경로로) / 끄기
pub fn set(enabled: bool) -> Result<(), String> {
    let key = wide(RUN_KEY);
    let name = wide(VALUE);
    unsafe {
        if !enabled {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            // 이미 없으면 성공으로 본다
            return if r == ERROR_SUCCESS || current().is_none() { Ok(()) } else { Err(format!("레지스트리 삭제 실패 ({r})")) };
        }
        let cmd = command_line().ok_or("실행 파일 경로를 알 수 없습니다")?;
        let data = wide(&cmd);
        let mut hkey: HKEY = std::ptr::null_mut();
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_SET_VALUE, &mut hkey);
        if r != ERROR_SUCCESS {
            return Err(format!("레지스트리 열기 실패 ({r})"));
        }
        let r = RegSetValueExW(hkey, name.as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32);
        RegCloseKey(hkey);
        if r != ERROR_SUCCESS {
            return Err(format!("레지스트리 쓰기 실패 ({r})"));
        }
    }
    Ok(())
}

/// 설정과 실제 등록을 맞춘다 (켜져 있는데 exe가 옮겨졌으면 경로 갱신)
pub fn sync(enabled: bool) -> Result<(), String> {
    match (enabled, current()) {
        (true, Some(v)) if Some(v.clone()) == command_line() => Ok(()),
        (true, _) => set(true),
        (false, Some(_)) => set(false),
        (false, None) => Ok(()),
    }
}
