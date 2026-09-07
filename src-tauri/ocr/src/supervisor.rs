use std::{
    ffi::OsString,
    io::{BufReader, ErrorKind},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::protocol::{
    read_response, validate_image, write_analyze, write_shutdown, AnalyzeResult, Failure, Ready,
    Response, STATUS_ERROR, STATUS_OK, STATUS_READY, VERSION,
};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const SHUTDOWN_ID: u64 = u64::MAX;

type Latest = Arc<Mutex<Option<Pending>>>;

#[derive(Clone)]
pub struct Config {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub startup_timeout: Duration,
    pub request_timeout: Duration,
    pub cancel_grace: Duration,
    pub idle_timeout: Duration,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum JobError {
    #[error("OCR 请求已被更新的请求替代")]
    Superseded,
    #[error("OCR 请求已取消")]
    Cancelled,
    #[error("OCR 辅助进程启动超时")]
    StartupTimeout,
    #[error("OCR 推理超时")]
    RequestTimeout,
    #[error("OCR 辅助进程已停止")]
    Stopped,
    #[error("OCR 辅助进程失败：{0}")]
    Process(String),
    #[error("OCR 协议失败：{0}")]
    Protocol(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ControlError {
    #[error("RGB 数据与图像尺寸不一致")]
    Image,
    #[error("OCR supervisor 已停止")]
    Stopped,
    #[error("OCR 取消确认超时")]
    CancelTimeout,
}

pub struct Job {
    id: u64,
    started: Receiver<()>,
    result: Receiver<Result<AnalyzeResult, JobError>>,
}

impl Job {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn wait_started(&self, timeout: Duration) -> Result<(), mpsc::RecvTimeoutError> {
        self.started.recv_timeout(timeout)
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Result<AnalyzeResult, JobError>, mpsc::RecvTimeoutError> {
        self.result.recv_timeout(timeout)
    }
}

pub struct Supervisor {
    latest: Latest,
    controls: mpsc::Sender<ControlMessage>,
    wake: SyncSender<()>,
    next_id: AtomicU64,
    activity: Arc<Activity>,
    worker: Option<JoinHandle<()>>,
}

impl Supervisor {
    pub fn start(config: Config) -> Self {
        Self::start_with_factory(Box::new(ProcessFactory { config }))
    }

    fn start_with_factory(factory: Box<dyn BackendFactory>) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let latest_for_worker = Arc::clone(&latest);
        let (controls, control_receiver) = mpsc::channel();
        let (wake, wake_receiver) = mpsc::sync_channel(1);
        let activity = Arc::new(Activity::default());
        let worker_activity = Arc::clone(&activity);
        let worker = thread::spawn(move || {
            manage(
                latest_for_worker,
                control_receiver,
                wake_receiver,
                worker_activity,
                factory,
            )
        });
        Self {
            latest,
            controls,
            wake,
            next_id: AtomicU64::new(1),
            activity,
            worker: Some(worker),
        }
    }

    pub fn submit(&self, width: u32, height: u32, rgb: Vec<u8>) -> Result<Job, ControlError> {
        let expected = validate_image(width, height).map_err(|_| ControlError::Image)?;
        if rgb.len() as u64 != expected {
            return Err(ControlError::Image);
        }
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| {
                (id < SHUTDOWN_ID).then(|| id + 1)
            })
            .map_err(|_| ControlError::Stopped)?;
        let (started_sender, started) = mpsc::channel();
        let (sender, result) = mpsc::channel();
        let pending = Pending {
            id,
            width,
            height,
            rgb,
            started: Some(started_sender),
            result: Some(sender),
            sent_at: None,
            cancel_deadline: None,
        };
        let mut latest = self.latest.lock().map_err(|_| ControlError::Stopped)?;
        if let Some(mut previous) = latest.replace(pending) {
            previous.finish(Err(JobError::Superseded));
        }
        drop(latest);
        if !self.notify() {
            self.fail_latest(id, JobError::Stopped);
            return Err(ControlError::Stopped);
        }
        Ok(Job {
            id,
            started,
            result,
        })
    }

    pub fn launch_count(&self) -> u64 {
        self.activity.launches.load(Ordering::Acquire)
    }

    pub fn process_id(&self) -> Option<u32> {
        match self.activity.pid.load(Ordering::Acquire) {
            0 => None,
            pid => Some(pid),
        }
    }

    pub fn cancel(&self, id: u64, timeout: Duration) -> Result<bool, ControlError> {
        {
            let mut latest = self.latest.lock().map_err(|_| ControlError::Stopped)?;
            if latest.as_ref().is_some_and(|pending| pending.id == id) {
                let mut pending = latest.take().expect("pending 已检查存在");
                pending.finish(Err(JobError::Cancelled));
                return Ok(true);
            }
        }
        let (sender, response) = mpsc::sync_channel(1);
        self.controls
            .send(ControlMessage::Cancel {
                id,
                response: sender,
            })
            .map_err(|_| ControlError::Stopped)?;
        self.notify();
        response.recv_timeout(timeout).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => ControlError::CancelTimeout,
            mpsc::RecvTimeoutError::Disconnected => ControlError::Stopped,
        })
    }

    pub fn shutdown(mut self, timeout: Duration) -> Result<(), ControlError> {
        self.stop(timeout)
    }

    fn stop(&mut self, timeout: Duration) -> Result<(), ControlError> {
        if self.worker.is_none() {
            return Ok(());
        }
        let (sender, response) = mpsc::sync_channel(1);
        if self
            .controls
            .send(ControlMessage::Stop { response: sender })
            .is_err()
        {
            return self.join_worker();
        }
        self.notify();
        match response.recv_timeout(timeout) {
            Ok(()) => self.join_worker(),
            Err(mpsc::RecvTimeoutError::Disconnected) => self.join_worker(),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(ControlError::CancelTimeout),
        }
    }

    fn notify(&self) -> bool {
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => true,
            Err(TrySendError::Disconnected(())) => false,
        }
    }

    fn fail_latest(&self, id: u64, error: JobError) {
        let Ok(mut latest) = self.latest.lock() else {
            return;
        };
        if latest.as_ref().is_some_and(|pending| pending.id == id) {
            if let Some(mut pending) = latest.take() {
                pending.finish(Err(error));
            }
        }
    }

    fn join_worker(&mut self) -> Result<(), ControlError> {
        self.worker
            .take()
            .expect("worker 已检查存在")
            .join()
            .map_err(|_| ControlError::Stopped)
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.stop(Duration::from_secs(2));
    }
}

enum ControlMessage {
    Cancel { id: u64, response: SyncSender<bool> },
    Stop { response: SyncSender<()> },
}

struct Pending {
    id: u64,
    width: u32,
    height: u32,
    rgb: Vec<u8>,
    started: Option<mpsc::Sender<()>>,
    result: Option<mpsc::Sender<Result<AnalyzeResult, JobError>>>,
    sent_at: Option<Instant>,
    cancel_deadline: Option<Instant>,
}

impl Pending {
    fn finish(&mut self, result: Result<AnalyzeResult, JobError>) {
        if let Some(sender) = self.result.take() {
            let _ = sender.send(result);
        }
    }

    fn cancel(&mut self, error: JobError, deadline: Instant) {
        self.finish(Err(error));
        self.cancel_deadline = Some(deadline);
    }
}

#[derive(Default)]
struct Activity {
    launches: AtomicU64,
    pid: AtomicU32,
}

struct State {
    activity: Arc<Activity>,
    process: Option<Box<dyn Backend>>,
    ready: bool,
    startup_deadline: Option<Instant>,
    current: Option<Pending>,
    idle_since: Option<Instant>,
    shutdown_deadline: Option<Instant>,
}

impl State {
    fn new(activity: Arc<Activity>) -> Self {
        Self {
            activity,
            process: None,
            ready: false,
            startup_deadline: None,
            current: None,
            idle_since: None,
            shutdown_deadline: None,
        }
    }

    fn stop_process(&mut self) {
        if let Some(mut process) = self.process.take() {
            let _ = process.stop();
        }
        self.activity.pid.store(0, Ordering::Release);
        self.ready = false;
        self.startup_deadline = None;
        self.idle_since = None;
        self.shutdown_deadline = None;
    }

    fn fail_current(&mut self, error: JobError) {
        if let Some(mut current) = self.current.take() {
            current.finish(Err(error));
        }
        self.stop_process();
    }
}

fn manage(
    latest: Latest,
    controls: Receiver<ControlMessage>,
    wake: Receiver<()>,
    activity: Arc<Activity>,
    mut factory: Box<dyn BackendFactory>,
) {
    let mut state = State::new(activity);
    loop {
        if handle_controls(&controls, &latest, &mut state, factory.cancel_grace()) {
            return;
        }
        observe_latest(&latest, &mut state, factory.cancel_grace());
        poll_response(&latest, &mut state);
        handle_timeouts(&latest, &mut state, factory.as_ref());
        drive(&latest, &mut state, factory.as_mut());
        if handle_controls(&controls, &latest, &mut state, factory.cancel_grace()) {
            return;
        }
        let _ = wake.recv_timeout(POLL_INTERVAL);
    }
}

fn handle_controls(
    controls: &Receiver<ControlMessage>,
    latest: &Latest,
    state: &mut State,
    cancel_grace: Duration,
) -> bool {
    loop {
        match controls.try_recv() {
            Ok(ControlMessage::Cancel { id, response }) => {
                let now = Instant::now();
                let cancelled = if state.current.as_ref().is_some_and(|job| job.id == id) {
                    if let Some(current) = state.current.as_mut() {
                        if current.result.is_some() {
                            current.cancel(JobError::Cancelled, now + cancel_grace);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    cancel_latest(latest, id)
                };
                let _ = response.send(cancelled);
            }
            Ok(ControlMessage::Stop { response }) => {
                fail_all(latest, state, JobError::Stopped);
                let _ = response.send(());
                return true;
            }
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                fail_all(latest, state, JobError::Stopped);
                return true;
            }
        }
    }
}

fn cancel_latest(latest: &Latest, id: u64) -> bool {
    let Ok(mut latest) = latest.lock() else {
        return false;
    };
    if latest.as_ref().is_some_and(|pending| pending.id == id) {
        if let Some(mut pending) = latest.take() {
            pending.finish(Err(JobError::Cancelled));
        }
        true
    } else {
        false
    }
}

fn observe_latest(latest: &Latest, state: &mut State, cancel_grace: Duration) {
    if !has_latest(latest) {
        return;
    }
    state.idle_since = None;
    if state.shutdown_deadline.is_some() {
        state.stop_process();
    }
    if let Some(current) = state.current.as_mut() {
        if current.result.is_some() {
            current.cancel(JobError::Superseded, Instant::now() + cancel_grace);
        }
    }
}

fn poll_response(latest: &Latest, state: &mut State) {
    let response = match state.process.as_mut() {
        Some(process) => process.try_response(),
        None => return,
    };
    let response = match response {
        Ok(Some(response)) => response,
        Ok(None) => return,
        Err(error) => {
            if state.ready {
                state.fail_current(JobError::Process(error));
            } else {
                fail_all(latest, state, JobError::Process(error));
            }
            return;
        }
    };

    if !state.ready {
        if response.status != STATUS_READY || response.id != 0 {
            fail_all(
                latest,
                state,
                JobError::Protocol("辅助进程未返回 ready 帧".into()),
            );
            return;
        }
        match serde_json::from_slice::<Ready>(&response.payload) {
            Ok(ready) if ready.protocol == VERSION => {
                state.ready = true;
                state.startup_deadline = None;
            }
            Ok(_) => fail_all(
                latest,
                state,
                JobError::Protocol("ready 协议版本不匹配".into()),
            ),
            Err(error) => fail_all(latest, state, JobError::Protocol(error.to_string())),
        }
        return;
    }

    if state.shutdown_deadline.is_some() {
        state.stop_process();
        return;
    }

    let Some(mut current) = state.current.take() else {
        state.stop_process();
        return;
    };
    if response.id != current.id {
        current.finish(Err(JobError::Protocol("响应 ID 不匹配".into())));
        state.stop_process();
        return;
    }
    if current.result.is_some() {
        current.finish(parse_result(response));
    }
    state.idle_since = (!has_latest(latest)).then(Instant::now);
}

fn parse_result(response: Response) -> Result<AnalyzeResult, JobError> {
    match response.status {
        STATUS_OK => serde_json::from_slice(&response.payload)
            .map_err(|error| JobError::Protocol(error.to_string())),
        STATUS_ERROR => serde_json::from_slice::<Failure<'_>>(&response.payload)
            .map(|failure| JobError::Process(format!("{}：{}", failure.code, failure.message)))
            .map_err(|error| JobError::Protocol(error.to_string()))
            .and_then(Err),
        status => Err(JobError::Protocol(format!("响应状态无效：{status}"))),
    }
}

fn handle_timeouts(latest: &Latest, state: &mut State, factory: &dyn BackendFactory) {
    let now = Instant::now();
    if state
        .startup_deadline
        .is_some_and(|deadline| now >= deadline)
    {
        fail_all(latest, state, JobError::StartupTimeout);
        return;
    }
    if let Some(current) = state.current.as_mut() {
        if current
            .cancel_deadline
            .is_some_and(|deadline| now >= deadline)
        {
            state.current.take();
            state.stop_process();
            return;
        }
        if current
            .sent_at
            .is_some_and(|sent_at| now.duration_since(sent_at) >= factory.request_timeout())
        {
            current.finish(Err(JobError::RequestTimeout));
            state.current.take();
            state.stop_process();
            return;
        }
    }
    if state
        .shutdown_deadline
        .is_some_and(|deadline| now >= deadline)
    {
        state.stop_process();
        return;
    }
    if state.ready
        && state.current.is_none()
        && !has_latest(latest)
        && state.shutdown_deadline.is_none()
        && state
            .idle_since
            .is_some_and(|idle| now.duration_since(idle) >= factory.idle_timeout())
    {
        match state
            .process
            .as_mut()
            .expect("ready process")
            .send_shutdown(SHUTDOWN_ID)
        {
            Ok(()) => state.shutdown_deadline = Some(now + factory.cancel_grace()),
            Err(_) => state.stop_process(),
        }
    }
}

fn drive(latest: &Latest, state: &mut State, factory: &mut dyn BackendFactory) {
    if state.process.is_none() && has_latest(latest) {
        match factory.spawn() {
            Ok(process) => {
                state.activity.launches.fetch_add(1, Ordering::Release);
                state.activity.pid.store(process.pid(), Ordering::Release);
                state.process = Some(process);
                state.ready = false;
                state.startup_deadline = Some(Instant::now() + factory.startup_timeout());
            }
            Err(error) => fail_latest(latest, JobError::Process(error)),
        }
        return;
    }
    if !state.ready || state.shutdown_deadline.is_some() || state.current.is_some() {
        return;
    }
    let Some(mut job) = take_latest(latest) else {
        state.idle_since.get_or_insert_with(Instant::now);
        return;
    };
    let result = state.process.as_mut().expect("ready process").send_analyze(
        job.id,
        job.width,
        job.height,
        std::mem::take(&mut job.rgb),
        job.started.take(),
    );
    match result {
        Ok(()) => {
            job.sent_at = Some(Instant::now());
            state.current = Some(job);
        }
        Err(error) => {
            job.finish(Err(JobError::Process(error)));
            state.stop_process();
        }
    }
}

fn has_latest(latest: &Latest) -> bool {
    latest.lock().is_ok_and(|pending| pending.is_some())
}

fn take_latest(latest: &Latest) -> Option<Pending> {
    latest.lock().ok()?.take()
}

fn fail_latest(latest: &Latest, error: JobError) {
    if let Some(mut pending) = take_latest(latest) {
        pending.finish(Err(error));
    }
}

fn fail_all(latest: &Latest, state: &mut State, error: JobError) {
    if let Some(mut current) = state.current.take() {
        current.finish(Err(error.clone()));
    }
    fail_latest(latest, error);
    state.stop_process();
}

trait BackendFactory: Send {
    fn spawn(&mut self) -> Result<Box<dyn Backend>, String>;
    fn startup_timeout(&self) -> Duration;
    fn request_timeout(&self) -> Duration;
    fn cancel_grace(&self) -> Duration;
    fn idle_timeout(&self) -> Duration;
}

trait Backend: Send {
    fn pid(&self) -> u32 {
        0
    }

    fn send_analyze(
        &mut self,
        id: u64,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        started: Option<mpsc::Sender<()>>,
    ) -> Result<(), String>;
    fn send_shutdown(&mut self, id: u64) -> Result<(), String>;
    fn try_response(&mut self) -> Result<Option<Response>, String>;
    fn stop(&mut self) -> Result<(), String>;
}

struct ProcessFactory {
    config: Config,
}

impl BackendFactory for ProcessFactory {
    fn spawn(&mut self) -> Result<Box<dyn Backend>, String> {
        ProcessBackend::spawn(&self.config).map(|backend| Box::new(backend) as Box<dyn Backend>)
    }

    fn startup_timeout(&self) -> Duration {
        self.config.startup_timeout
    }

    fn request_timeout(&self) -> Duration {
        self.config.request_timeout
    }

    fn cancel_grace(&self) -> Duration {
        self.config.cancel_grace
    }

    fn idle_timeout(&self) -> Duration {
        self.config.idle_timeout
    }
}

enum WriteCommand {
    Analyze {
        id: u64,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        started: Option<mpsc::Sender<()>>,
    },
    Shutdown {
        id: u64,
    },
}

struct ProcessBackend {
    child: Child,
    writes: Option<SyncSender<WriteCommand>>,
    responses: Receiver<Result<Response, String>>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    stopped: bool,
}

impl ProcessBackend {
    fn spawn(config: &Config) -> Result<Self, String> {
        let mut command = Command::new(&config.program);
        command
            .args(&config.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // The host is a GUI process on Windows; without this flag every
        // sidecar launch would flash a console window.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdin = child.stdin.take().ok_or("sidecar stdin 不可用")?;
        let stdout = child.stdout.take().ok_or("sidecar stdout 不可用")?;
        let (response_sender, responses) = mpsc::channel();
        let reader_sender = response_sender.clone();
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_response(&mut reader) {
                    Ok(Some(response)) => {
                        if reader_sender.send(Ok(response)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {
                        let _ = reader_sender.send(Err("sidecar stdout 已关闭".into()));
                        return;
                    }
                    Err(error) => {
                        let _ = reader_sender.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        });
        let (writes, write_receiver) = mpsc::sync_channel(1);
        let writer = thread::spawn(move || write_loop(stdin, write_receiver, response_sender));
        Ok(Self {
            child,
            writes: Some(writes),
            responses,
            writer: Some(writer),
            reader: Some(reader),
            stopped: false,
        })
    }
}

fn write_loop(
    mut stdin: ChildStdin,
    commands: Receiver<WriteCommand>,
    responses: mpsc::Sender<Result<Response, String>>,
) {
    while let Ok(command) = commands.recv() {
        let result = match command {
            WriteCommand::Analyze {
                id,
                width,
                height,
                rgb,
                started,
            } => write_analyze(&mut stdin, id, width, height, &rgb).map(|()| {
                if let Some(started) = started {
                    let _ = started.send(());
                }
            }),
            WriteCommand::Shutdown { id } => write_shutdown(&mut stdin, id),
        };
        if let Err(error) = result {
            let _ = responses.send(Err(error.to_string()));
            return;
        }
    }
}

impl Backend for ProcessBackend {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send_analyze(
        &mut self,
        id: u64,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        started: Option<mpsc::Sender<()>>,
    ) -> Result<(), String> {
        self.writes
            .as_ref()
            .ok_or("sidecar 写入通道已关闭")?
            .try_send(WriteCommand::Analyze {
                id,
                width,
                height,
                rgb,
                started,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => "sidecar 写入队列已满".to_owned(),
                TrySendError::Disconnected(_) => "sidecar 写入通道已关闭".to_owned(),
            })
    }

    fn send_shutdown(&mut self, id: u64) -> Result<(), String> {
        self.writes
            .as_ref()
            .ok_or("sidecar 写入通道已关闭")?
            .try_send(WriteCommand::Shutdown { id })
            .map_err(|error| match error {
                TrySendError::Full(_) => "sidecar 写入队列已满".to_owned(),
                TrySendError::Disconnected(_) => "sidecar 写入通道已关闭".to_owned(),
            })
    }

    fn try_response(&mut self) -> Result<Option<Response>, String> {
        match self.responses.try_recv() {
            Ok(response) => response.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("sidecar 响应通道已关闭".into()),
        }
    }

    fn stop(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.writes.take();
        if self
            .child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_none()
        {
            match self.child.kill() {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::InvalidInput => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        self.child.wait().map_err(|error| error.to_string())?;
        self.stopped = true;
        if let Some(writer) = self.writer.take() {
            writer
                .join()
                .map_err(|_| "请求写入线程异常退出".to_owned())?;
        }
        if let Some(reader) = self.reader.take() {
            reader
                .join()
                .map_err(|_| "响应读取线程异常退出".to_owned())?;
        }
        Ok(())
    }
}

impl Drop for ProcessBackend {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, sync::Arc};

    use super::*;
    use crate::protocol::{write_response, StageTimes};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Event {
        Launch,
        Analyze(u64),
        Shutdown,
        Stop,
    }

    struct FakeFactory {
        events: Arc<Mutex<Vec<Event>>>,
        ready_delay: Duration,
        result_delay: Duration,
        startup_timeout: Duration,
        request_timeout: Duration,
        cancel_grace: Duration,
        idle_timeout: Duration,
        shutdown_response: bool,
    }

    impl BackendFactory for FakeFactory {
        fn spawn(&mut self) -> Result<Box<dyn Backend>, String> {
            self.events.lock().unwrap().push(Event::Launch);
            Ok(Box::new(FakeBackend {
                events: Arc::clone(&self.events),
                responses: VecDeque::from([(
                    Instant::now() + self.ready_delay,
                    response(
                        STATUS_READY,
                        0,
                        &Ready {
                            protocol: VERSION,
                            engine: "fake".into(),
                            init_ms: 0,
                            runtime: "fake".into(),
                        },
                    ),
                )]),
                result_delay: self.result_delay,
                shutdown_response: self.shutdown_response,
                stopped: false,
            }))
        }

        fn startup_timeout(&self) -> Duration {
            self.startup_timeout
        }

        fn request_timeout(&self) -> Duration {
            self.request_timeout
        }

        fn cancel_grace(&self) -> Duration {
            self.cancel_grace
        }

        fn idle_timeout(&self) -> Duration {
            self.idle_timeout
        }
    }

    struct FakeBackend {
        events: Arc<Mutex<Vec<Event>>>,
        responses: VecDeque<(Instant, Response)>,
        result_delay: Duration,
        shutdown_response: bool,
        stopped: bool,
    }

    impl Backend for FakeBackend {
        fn send_analyze(
            &mut self,
            id: u64,
            width: u32,
            height: u32,
            _rgb: Vec<u8>,
            started: Option<mpsc::Sender<()>>,
        ) -> Result<(), String> {
            if let Some(started) = started {
                let _ = started.send(());
            }
            self.events.lock().unwrap().push(Event::Analyze(id));
            self.responses.push_back((
                Instant::now() + self.result_delay,
                response(
                    STATUS_OK,
                    id,
                    &AnalyzeResult {
                        width,
                        height,
                        elapsed_ms: 1,
                        stages: StageTimes::default(),
                        lines: Vec::new(),
                    },
                ),
            ));
            Ok(())
        }

        fn send_shutdown(&mut self, id: u64) -> Result<(), String> {
            self.events.lock().unwrap().push(Event::Shutdown);
            if self.shutdown_response {
                self.responses.push_back((
                    Instant::now(),
                    response(STATUS_OK, id, &serde_json::json!({"stopped": true})),
                ));
            }
            Ok(())
        }

        fn try_response(&mut self) -> Result<Option<Response>, String> {
            if self
                .responses
                .front()
                .is_some_and(|(ready_at, _)| Instant::now() >= *ready_at)
            {
                return Ok(self.responses.pop_front().map(|(_, response)| response));
            }
            Ok(None)
        }

        fn stop(&mut self) -> Result<(), String> {
            if !self.stopped {
                self.events.lock().unwrap().push(Event::Stop);
                self.stopped = true;
            }
            Ok(())
        }
    }

    fn response<T: serde::Serialize>(status: u16, id: u64, payload: &T) -> Response {
        let mut bytes = Vec::new();
        write_response(&mut bytes, status, id, payload).unwrap();
        read_response(&mut bytes.as_slice()).unwrap().unwrap()
    }

    fn supervisor(
        ready_delay: Duration,
        result_delay: Duration,
        cancel_grace: Duration,
        idle_timeout: Duration,
        shutdown_response: bool,
    ) -> (Supervisor, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let factory = FakeFactory {
            events: Arc::clone(&events),
            ready_delay,
            result_delay,
            startup_timeout: Duration::from_millis(500),
            request_timeout: Duration::from_secs(2),
            cancel_grace,
            idle_timeout,
            shutdown_response,
        };
        (Supervisor::start_with_factory(Box::new(factory)), events)
    }

    fn standard_supervisor(
        ready_delay: Duration,
        result_delay: Duration,
        cancel_grace: Duration,
        idle_timeout: Duration,
    ) -> (Supervisor, Arc<Mutex<Vec<Event>>>) {
        supervisor(ready_delay, result_delay, cancel_grace, idle_timeout, true)
    }

    fn wait_for(events: &Arc<Mutex<Vec<Event>>>, expected: Event) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if events.lock().unwrap().contains(&expected) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("未观察到事件：{expected:?}");
    }

    struct FailingFactory {
        events: Arc<Mutex<Vec<Event>>>,
    }

    impl BackendFactory for FailingFactory {
        fn spawn(&mut self) -> Result<Box<dyn Backend>, String> {
            self.events.lock().unwrap().push(Event::Launch);
            Ok(Box::new(FailingBackend {
                events: Arc::clone(&self.events),
            }))
        }

        fn startup_timeout(&self) -> Duration {
            Duration::from_millis(500)
        }

        fn request_timeout(&self) -> Duration {
            Duration::from_secs(2)
        }

        fn cancel_grace(&self) -> Duration {
            Duration::from_millis(40)
        }

        fn idle_timeout(&self) -> Duration {
            Duration::from_secs(5)
        }
    }

    struct FailingBackend {
        events: Arc<Mutex<Vec<Event>>>,
    }

    impl Backend for FailingBackend {
        fn send_analyze(
            &mut self,
            _id: u64,
            _width: u32,
            _height: u32,
            _rgb: Vec<u8>,
            _started: Option<mpsc::Sender<()>>,
        ) -> Result<(), String> {
            Err("unexpected analyze".into())
        }

        fn send_shutdown(&mut self, _id: u64) -> Result<(), String> {
            Ok(())
        }

        fn try_response(&mut self) -> Result<Option<Response>, String> {
            Err("startup failed".into())
        }

        fn stop(&mut self) -> Result<(), String> {
            self.events.lock().unwrap().push(Event::Stop);
            Ok(())
        }
    }

    #[test]
    fn startup_failure_fails_pending_request_without_restart_loop() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let supervisor = Supervisor::start_with_factory(Box::new(FailingFactory {
            events: Arc::clone(&events),
        }));
        let job = supervisor.submit(1, 1, vec![1; 3]).unwrap();

        assert_eq!(
            job.recv_timeout(Duration::from_millis(200)).unwrap(),
            Err(JobError::Process("startup failed".into()))
        );
        wait_for(&events, Event::Stop);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| **event == Event::Launch)
                .count(),
            1
        );
    }

    #[test]
    fn keeps_only_the_latest_pending_request() {
        let (supervisor, events) = standard_supervisor(
            Duration::ZERO,
            Duration::from_millis(80),
            Duration::from_millis(200),
            Duration::from_secs(5),
        );
        let first = supervisor.submit(1, 1, vec![1; 3]).unwrap();
        wait_for(&events, Event::Analyze(first.id()));
        let second = supervisor.submit(1, 1, vec![2; 3]).unwrap();
        let third = supervisor.submit(1, 1, vec![3; 3]).unwrap();

        assert_eq!(
            first.recv_timeout(Duration::from_millis(200)).unwrap(),
            Err(JobError::Superseded)
        );
        assert_eq!(
            second.recv_timeout(Duration::from_millis(200)).unwrap(),
            Err(JobError::Superseded)
        );
        assert!(third
            .recv_timeout(Duration::from_millis(500))
            .unwrap()
            .is_ok());
        let analyzed = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::Analyze(id) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(analyzed, vec![first.id(), third.id()]);
    }

    #[test]
    fn cancellation_returns_immediately_and_reaps_after_grace() {
        let (supervisor, events) = standard_supervisor(
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_millis(40),
            Duration::from_secs(5),
        );
        let job = supervisor.submit(1, 1, vec![1; 3]).unwrap();
        wait_for(&events, Event::Analyze(job.id()));
        let started = Instant::now();
        assert!(supervisor
            .cancel(job.id(), Duration::from_millis(200))
            .unwrap());
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(
            job.recv_timeout(Duration::from_millis(200)).unwrap(),
            Err(JobError::Cancelled)
        );
        wait_for(&events, Event::Stop);
    }

    #[test]
    fn exits_after_idle_timeout() {
        let (supervisor, events) = standard_supervisor(
            Duration::ZERO,
            Duration::from_millis(5),
            Duration::from_millis(40),
            Duration::from_millis(30),
        );
        let job = supervisor.submit(1, 1, vec![1; 3]).unwrap();
        assert!(job
            .recv_timeout(Duration::from_millis(200))
            .unwrap()
            .is_ok());
        wait_for(&events, Event::Shutdown);
        wait_for(&events, Event::Stop);
    }

    #[test]
    fn reaps_when_idle_shutdown_does_not_respond() {
        let (supervisor, events) = supervisor(
            Duration::ZERO,
            Duration::from_millis(5),
            Duration::from_millis(30),
            Duration::from_millis(20),
            false,
        );
        let job = supervisor.submit(1, 1, vec![1; 3]).unwrap();
        assert!(job
            .recv_timeout(Duration::from_millis(200))
            .unwrap()
            .is_ok());
        wait_for(&events, Event::Shutdown);
        wait_for(&events, Event::Stop);
    }

    #[test]
    fn explicit_shutdown_reaps_the_process() {
        let (supervisor, events) = standard_supervisor(
            Duration::ZERO,
            Duration::from_millis(5),
            Duration::from_millis(40),
            Duration::from_secs(5),
        );
        let job = supervisor.submit(1, 1, vec![1; 3]).unwrap();
        assert!(job
            .recv_timeout(Duration::from_millis(200))
            .unwrap()
            .is_ok());

        supervisor.shutdown(Duration::from_millis(200)).unwrap();
        wait_for(&events, Event::Stop);
    }

    #[test]
    fn stop_is_not_blocked_by_repeated_submissions() {
        let (supervisor, events) = standard_supervisor(
            Duration::from_secs(1),
            Duration::from_millis(5),
            Duration::from_millis(40),
            Duration::from_secs(5),
        );
        let mut latest = supervisor.submit(1, 1, vec![0; 3]).unwrap();
        wait_for(&events, Event::Launch);
        for value in 1..100 {
            latest = supervisor.submit(1, 1, vec![value; 3]).unwrap();
        }
        let started = Instant::now();
        supervisor.shutdown(Duration::from_millis(200)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(
            latest.recv_timeout(Duration::from_millis(200)).unwrap(),
            Err(JobError::Stopped)
        );
        wait_for(&events, Event::Stop);
    }

    #[test]
    fn supersedes_requests_while_starting() {
        let (supervisor, events) = standard_supervisor(
            Duration::from_millis(60),
            Duration::from_millis(5),
            Duration::from_millis(40),
            Duration::from_secs(5),
        );
        let first = supervisor.submit(1, 1, vec![1; 3]).unwrap();
        let second = supervisor.submit(1, 1, vec![2; 3]).unwrap();
        assert_eq!(
            first.recv_timeout(Duration::from_millis(200)).unwrap(),
            Err(JobError::Superseded)
        );
        assert!(second
            .recv_timeout(Duration::from_millis(500))
            .unwrap()
            .is_ok());
        let analyzed = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                Event::Analyze(id) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(analyzed, vec![second.id()]);
    }
}
