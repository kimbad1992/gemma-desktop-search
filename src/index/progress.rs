//! 인덱싱 진행 상태 (UI가 매 프레임 읽어 표시)

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running,
    Paused,
    Stopping,
    Stopped,
    Completed,
    Failed,
}

impl RunState {
    pub fn label(self) -> &'static str {
        match self {
            RunState::Idle => "대기",
            RunState::Running => "인덱싱 진행 중",
            RunState::Paused => "일시정지",
            RunState::Stopping => "중지하는 중…",
            RunState::Stopped => "중지됨",
            RunState::Completed => "완료",
            RunState::Failed => "오류",
        }
    }
    pub fn is_active(self) -> bool {
        matches!(self, RunState::Running | RunState::Paused | RunState::Stopping)
    }
}

pub const MAX_ERROR_LIST: usize = 1000;

#[derive(Clone)]
pub struct Progress {
    pub state: RunState,
    /// 폴더 탐색이 아직 진행 중 (전체 개수 미확정)
    pub scanning: bool,
    pub full_rebuild: bool,
    /// 폴더 감시가 알려 준 바뀐 파일만 처리하는 실행
    pub targeted: bool,
    pub discovered: u64,
    pub discovered_by_kind: [u64; 6],
    /// 결과와 상관없이 처리가 끝난 파일 수
    pub processed: u64,
    pub indexed_by_kind: [u64; 6],
    pub unchanged: u64,
    pub name_only: u64,
    pub errors: u64,
    pub warnings: u64,
    pub inaccessible: u64,
    pub chunks: u64,
    pub removed: u64,
    /// 옮기거나 이름만 바꿔서, 내용은 다시 읽지 않고 경로만 고친 파일 수
    pub moved: u64,
    pub current: String,
    pub started: Option<Instant>,
    pub finished: Option<Instant>,
    pub paused_since: Option<Instant>,
    pub paused_total: Duration,
    pub message: String,
    pub error_list: Vec<(String, String)>,
    samples: VecDeque<(Instant, u64, u64)>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            state: RunState::Idle,
            scanning: false,
            full_rebuild: false,
            targeted: false,
            discovered: 0,
            discovered_by_kind: [0; 6],
            processed: 0,
            indexed_by_kind: [0; 6],
            unchanged: 0,
            name_only: 0,
            errors: 0,
            warnings: 0,
            inaccessible: 0,
            chunks: 0,
            removed: 0,
            moved: 0,
            current: String::new(),
            started: None,
            finished: None,
            paused_since: None,
            paused_total: Duration::ZERO,
            message: String::new(),
            error_list: Vec::new(),
            samples: VecDeque::new(),
        }
    }
}

impl Progress {
    pub fn elapsed(&self) -> Duration {
        let Some(start) = self.started else {
            return Duration::ZERO;
        };
        let end = self.finished.unwrap_or_else(Instant::now);
        let mut paused = self.paused_total;
        if let Some(p) = self.paused_since {
            paused += p.elapsed();
        }
        end.saturating_duration_since(start).saturating_sub(paused)
    }

    /// 최근 구간의 처리 속도: (파일/초, 청크/초)
    pub fn rates(&self) -> (f64, f64) {
        if self.state != RunState::Running {
            return (0.0, 0.0);
        }
        match (self.samples.front(), self.samples.back()) {
            (Some(a), Some(b)) if b.0 > a.0 => {
                let dt = (b.0 - a.0).as_secs_f64();
                if dt < 0.5 {
                    return (0.0, 0.0);
                }
                ((b.1 - a.1) as f64 / dt, (b.2 - a.2) as f64 / dt)
            }
            _ => (0.0, 0.0),
        }
    }

    pub fn eta(&self) -> Option<Duration> {
        if self.scanning || self.state != RunState::Running {
            return None;
        }
        let (rate, _) = self.rates();
        if rate <= 0.01 {
            return None;
        }
        let remaining = self.discovered.saturating_sub(self.processed) as f64;
        Some(Duration::from_secs_f64(remaining / rate))
    }

    /// 탐색이 끝난 뒤에만 비율 계산
    pub fn fraction(&self) -> Option<f32> {
        if self.scanning || self.discovered == 0 {
            return None;
        }
        Some((self.processed as f32 / self.discovered as f32).clamp(0.0, 1.0))
    }

    pub fn sample(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.samples.back() {
            if now.duration_since(last.0) < Duration::from_millis(250) {
                return;
            }
        }
        self.samples.push_back((now, self.processed, self.chunks));
        while let Some(f) = self.samples.front() {
            if now.duration_since(f.0) > Duration::from_secs(12) && self.samples.len() > 2 {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn add_error(&mut self, path: &str, msg: impl Into<String>) {
        self.errors += 1;
        if self.error_list.len() < MAX_ERROR_LIST {
            self.error_list.push((path.to_string(), msg.into()));
        }
    }

    /// 파일은 인덱싱됐지만 일부 구간이 빠진 경우 (오류 수에는 포함하지 않음)
    pub fn add_warning(&mut self, path: &str, msg: impl Into<String>) {
        self.warnings += 1;
        if self.error_list.len() < MAX_ERROR_LIST {
            self.error_list.push((path.to_string(), format!("[경고] {}", msg.into())));
        }
    }

    pub fn reset_for_run(&mut self, full_rebuild: bool) {
        *self = Progress {
            state: RunState::Running,
            scanning: true,
            full_rebuild,
            started: Some(Instant::now()),
            ..Default::default()
        };
    }
}
