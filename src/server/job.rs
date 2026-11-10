//! Windows Job Object — 앱이 비정상 종료돼도 자식 프로세스(llama-server, ffmpeg)가 함께 종료되도록.
//! Linux에서는 `platform::die_with_parent`가 같은 일을 한다.

/// 자식 프로세스를 앱에 묶는다. Windows가 아니면 할 일이 없다.
#[cfg(not(windows))]
pub fn adopt(_child: &std::process::Child) -> bool {
    true
}

#[cfg(windows)]
pub use win::adopt;

#[cfg(windows)]
mod win {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(usize);

    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    fn job() -> Option<&'static Job> {
        JOB.get_or_init(|| unsafe {
            let h: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(h);
                return None;
            }
            // 핸들은 일부러 닫지 않는다: 앱 프로세스가 끝날 때 OS가 닫으면서 Job 안의 프로세스를 모두 종료.
            Some(Job(h as usize))
        })
        .as_ref()
    }

    /// 자식 프로세스를 앱의 Job에 넣는다. 실패해도 동작에는 지장 없음(정리 보장만 약해짐).
    pub fn adopt(child: &Child) -> bool {
        match job() {
            Some(j) => unsafe {
                AssignProcessToJobObject(j.0 as HANDLE, child.as_raw_handle() as HANDLE) != 0
            },
            None => false,
        }
    }
}
