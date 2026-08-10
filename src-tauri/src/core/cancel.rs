//! 运行中任务的停止开关。
//!
//! 管道跑在后台任务里，命令一发出就立刻返回，之后没有任何句柄能碰到它——
//! 想中断就得在别处留一个把手。这里存两样东西：
//!
//! * `cancelled` 标志：管道自己在关键点回头看一眼，以及收尾时判断
//!   「结果还要不要写回数据库」（用户已经按停止了就不能再盖成 succeeded）。
//! * `abort`：真正把后台任务掐掉。只靠标志的话，一次出图请求可能要等
//!   十几分钟才走到下一个检查点，用户按了停止却什么也没发生。
//!
//! 注意 abort 只在本地丢弃这个 future，上游那一次已经发出的请求该收的钱照收，
//! 停止能省下的是它后面还没发出的那些。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct JobControl {
    cancelled: Arc<AtomicBool>,
    abort: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
pub struct CancelRegistry {
    // 只在注册/停止/收尾三处短暂上锁，锁里不做 IO
    entries: Mutex<HashMap<String, JobControl>>,
}

impl CancelRegistry {
    pub fn register(
        &self,
        job_id: &str,
        cancelled: Arc<AtomicBool>,
        abort: impl Fn() + Send + Sync + 'static,
    ) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                job_id.to_string(),
                JobControl {
                    cancelled,
                    abort: Box::new(abort),
                },
            );
        }
    }

    /// 任务自己跑完了：把把手取下来，别让 map 无限长。
    pub fn finish(&self, job_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(job_id);
        }
    }

    /// 返回是否真的掐到了一个在跑的任务（没有登记说明它已经结束了）。
    pub fn cancel(&self, job_id: &str) -> bool {
        let control = match self.entries.lock() {
            Ok(mut entries) => entries.remove(job_id),
            Err(_) => None,
        };
        match control {
            Some(control) => {
                // 先立标志再 abort：任务若正好走到收尾，它会看到标志而不写结果
                control.cancelled.store(true, Ordering::SeqCst);
                (control.abort)();
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_sets_the_flag_and_fires_abort_once() {
        let registry = CancelRegistry::default();
        let flag = Arc::new(AtomicBool::new(false));
        let aborted = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&aborted);
        registry.register("job-1", Arc::clone(&flag), move || {
            seen.store(true, Ordering::SeqCst)
        });

        assert!(registry.cancel("job-1"));
        assert!(flag.load(Ordering::SeqCst));
        assert!(aborted.load(Ordering::SeqCst));
        // 第二次停止无事可做：句柄已经取下来了，不能再 abort 一个不存在的任务
        assert!(!registry.cancel("job-1"));
    }

    #[test]
    fn finished_jobs_are_no_longer_cancellable() {
        let registry = CancelRegistry::default();
        registry.register("job-2", Arc::new(AtomicBool::new(false)), || {});
        registry.finish("job-2");
        assert!(!registry.cancel("job-2"));
    }
}
