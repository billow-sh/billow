use crate::multipass;
use std::collections::{HashSet, VecDeque};
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const POOL_SIZE: usize = 1;
const VM_PREFIX: &str = "billow-pool";
const STOP_LAUNCH_SETTLE_TIMEOUT: Duration = Duration::from_secs(780);

pub(crate) struct Pool {
    state: Mutex<State>,
    ready_changed: Condvar,
}

struct State {
    ready: VecDeque<String>,
    taken: HashSet<String>,
    deleting: HashSet<String>,
    launching: Option<String>,
    next_id: u64,
    shutdown: bool,
}

impl Pool {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                ready: VecDeque::new(),
                taken: HashSet::new(),
                deleting: HashSet::new(),
                launching: None,
                next_id: 0,
                shutdown: false,
            }),
            ready_changed: Condvar::new(),
        }
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        self.state.lock().expect("pool state poisoned").shutdown
    }

    pub(crate) fn request_shutdown(&self) {
        let mut state = self.state.lock().expect("pool state poisoned");
        state.shutdown = true;
        self.ready_changed.notify_all();
    }

    pub(crate) fn launcher_loop(&self) {
        loop {
            let Some(vm_name) = self.reserve_launch_slot() else {
                return;
            };

            let result = self.launch_vm(&vm_name);
            let launched = result.is_ok();
            if let Err(error) = result {
                eprintln!("vm-pool failed to launch {vm_name}: {error}");
            }

            {
                let mut state = self.state.lock().expect("pool state poisoned");
                if state.launching.as_deref() == Some(vm_name.as_str()) {
                    state.launching = None;
                    if launched {
                        eprintln!("vm-pool ready: {vm_name}");
                        state.ready.push_back(vm_name.clone());
                    }
                }

                self.ready_changed.notify_all();
            }

            if !launched && !self.is_shutdown() {
                self.sleep_unless_shutdown(Duration::from_secs(5));
            }
        }
    }

    fn reserve_launch_slot(&self) -> Option<String> {
        let mut state = self.state.lock().expect("pool state poisoned");

        loop {
            if state.shutdown {
                return None;
            }

            let warming = state.ready.len() + usize::from(state.launching.is_some());
            if warming < POOL_SIZE {
                state.next_id += 1;
                let vm_name = next_vm_name(state.next_id);
                state.launching = Some(vm_name.clone());
                self.ready_changed.notify_all();
                return Some(vm_name);
            }

            state = self.ready_changed.wait(state).expect("pool state poisoned");
        }
    }

    fn launch_vm(&self, vm_name: &str) -> io::Result<()> {
        eprintln!("vm-pool launching: {vm_name}");

        multipass::launch_vm(vm_name)?;

        if let Err(error) = self.wait_for_vm_exec(vm_name) {
            multipass::destroy_vm(vm_name);
            return Err(error);
        }

        Ok(())
    }

    fn wait_for_vm_exec(&self, vm_name: &str) -> io::Result<()> {
        for _ in 0..60 {
            if multipass::vm_exec_ready(vm_name) {
                return Ok(());
            }

            thread::sleep(Duration::from_secs(2));
        }

        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{vm_name} did not become ready for multipass exec"),
        ))
    }

    pub(crate) fn take(&self) -> io::Result<String> {
        let mut state = self.state.lock().expect("pool state poisoned");

        loop {
            if state.shutdown {
                return Err(io::Error::other("vm-pool is shutting down"));
            }

            if let Some(vm_name) = state.ready.pop_front() {
                state.taken.insert(vm_name.clone());
                self.ready_changed.notify_all();
                return Ok(vm_name);
            }

            state = self.ready_changed.wait(state).expect("pool state poisoned");
        }
    }

    pub(crate) fn drop_vm(self: &Arc<Self>, vm_name: &str) -> io::Result<()> {
        let mut state = self.state.lock().expect("pool state poisoned");
        if !state.taken.remove(vm_name) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{vm_name} is not taken by vm-pool"),
            ));
        }

        state.deleting.insert(vm_name.to_string());
        self.ready_changed.notify_all();
        drop(state);

        let pool = Arc::clone(self);
        let vm_name = vm_name.to_string();
        thread::spawn(move || {
            multipass::destroy_vm(&vm_name);
            let mut state = pool.state.lock().expect("pool state poisoned");
            state.deleting.remove(&vm_name);
            pool.ready_changed.notify_all();
        });

        Ok(())
    }

    pub(crate) fn stop_all(&self) -> String {
        let mut vms = Vec::new();
        {
            let mut state = self.state.lock().expect("pool state poisoned");
            state.shutdown = true;
            vms.extend(state.ready.drain(..));
            vms.extend(state.taken.drain());
            vms.extend(state.deleting.drain());
            self.ready_changed.notify_all();
        }

        vms.sort();
        vms.dedup();

        for vm_name in &vms {
            multipass::destroy_vm(vm_name);
        }

        if !self.wait_for_cleanup(STOP_LAUNCH_SETTLE_TIMEOUT) {
            eprintln!(
                "vm-pool timed out waiting for launching VM to settle; attempting best-effort cleanup"
            );
        }

        let mut stragglers = self.drain_known_vms();
        stragglers.sort();
        stragglers.dedup();

        for vm_name in &stragglers {
            multipass::destroy_vm(vm_name);
        }

        multipass::purge();

        format!("stopped {}", vms.len() + stragglers.len())
    }

    fn wait_for_cleanup(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().expect("pool state poisoned");

        while (state.launching.is_some() || !state.deleting.is_empty()) && Instant::now() < deadline
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait = remaining.min(Duration::from_secs(1));
            let (next_state, _) = self
                .ready_changed
                .wait_timeout(state, wait)
                .expect("pool state poisoned");
            state = next_state;
        }

        state.launching.is_none() && state.deleting.is_empty()
    }

    fn drain_known_vms(&self) -> Vec<String> {
        let mut state = self.state.lock().expect("pool state poisoned");
        let mut vms = Vec::new();
        vms.extend(state.ready.drain(..));
        vms.extend(state.taken.drain());
        vms.extend(state.deleting.drain());
        if let Some(vm_name) = state.launching.take() {
            vms.push(vm_name);
        }
        vms
    }

    pub(crate) fn status(&self) -> String {
        let state = self.state.lock().expect("pool state poisoned");
        format!(
            "ready={} taken={} deleting={} launching={} shutdown={}",
            state.ready.len(),
            state.taken.len(),
            state.deleting.len(),
            usize::from(state.launching.is_some()),
            state.shutdown
        )
    }

    pub(crate) fn wait_ready(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.lock().expect("pool state poisoned");

        loop {
            if state.shutdown {
                return Err(io::Error::other("vm-pool is shutting down"));
            }

            if !state.ready.is_empty() {
                return Ok(());
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for a ready VM",
                ));
            }

            let (next_state, timeout) = self
                .ready_changed
                .wait_timeout(state, remaining)
                .expect("pool state poisoned");
            state = next_state;

            if timeout.timed_out() && state.ready.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for a ready VM",
                ));
            }
        }
    }

    fn sleep_unless_shutdown(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        let mut state = self.state.lock().expect("pool state poisoned");

        while !state.shutdown && Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait = remaining.min(Duration::from_millis(500));
            let (next_state, _) = self
                .ready_changed
                .wait_timeout(state, wait)
                .expect("pool state poisoned");
            state = next_state;
        }
    }
}

fn next_vm_name(next_id: u64) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    format!("{VM_PREFIX}-{timestamp}-{}-{next_id}", std::process::id())
}
