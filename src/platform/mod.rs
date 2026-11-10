//! OS에 묶인 작은 기능을 한곳에 모은다. OS마다 같은 이름의 함수를 구현한다.
//!
//! - `windows.rs`: 전부 구현돼 있다.
//! - `unix.rs`: Linux용. 창 없는 서비스판에 필요한 것만 있다 (창·탐색기·클립보드 기능은 아직 없다).
//!
//! 이 폴더 밖에서 OS에 묶인 것은 파일 단위로 떨어져 있는 아래 셋이고, 화면이 있는 앱에서만 쓴다.
//! - `shell.rs`: 트레이 아이콘, 전역 단축키, 한 번만 실행, 전체 화면 감지, 작업 영역
//! - `autostart.rs`: 로그인 시 실행
//! - `server/job.rs`: 앱이 끝나면 자식 프로세스(llama-server)도 끝나게 묶기 (Windows. Linux는 `die_with_parent`)
//!
//! 그 밖에 OS마다 달라지는 것: 업데이트의 실행 파일 교체(`update.rs`), 받을 llama.cpp 판(`download.rs`),
//! 위치를 넘겨 여는 앱 목록(`open.rs`).
//!
//! 새 기능에서 OS 기능이 필요하면 다른 파일에서 직접 부르지 말고 여기에 함수를 추가한다.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
