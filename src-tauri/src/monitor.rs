//! Periodic CPU / memory / GPU statistics for the status bar and the resource view.
//! GPU figures come from NVML (loaded at runtime from the NVIDIA driver); without it
//! the GPU section is simply empty. Only statistics are queried; no GPU work is started.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::enums::device::UsedGpuMemory;
use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use sakiyomi_app::session::Manager;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    name: String,
    util: u32,
    mem_util: u32,
    mem_used: u64,
    mem_total: u64,
    temp: Option<u32>,
    power_w: Option<f32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EngineProcStats {
    session: u32,
    name: String,
    pid: u32,
    /// Share of the whole machine's CPU, 0–100.
    cpu: f32,
    mem_bytes: u64,
    gpu_util: Option<u32>,
    gpu_mem: Option<u64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemStats {
    cpu: f32,
    cores: usize,
    mem_used: u64,
    mem_total: u64,
    gpus: Vec<GpuStats>,
    gpu_error: Option<String>,
    engines: Vec<EngineProcStats>,
}

struct Gpu {
    nvml: Option<Nvml>,
    error: Option<String>,
    /// NVML timestamp of the last per-process utilization sample.
    last_seen: u64,
}

impl Gpu {
    fn new() -> Self {
        match Nvml::init() {
            Ok(nvml) => Gpu { nvml: Some(nvml), error: None, last_seen: 0 },
            // Raw error; the UI adds the localised explanation.
            Err(e) => Gpu { nvml: None, error: Some(e.to_string()), last_seen: 0 },
        }
    }

    /// Device stats plus per-process (pid → util %, memory bytes) for all devices.
    fn sample(&mut self) -> (Vec<GpuStats>, HashMap<u32, (u32, u64)>) {
        let mut out = Vec::new();
        let mut procs: HashMap<u32, (u32, u64)> = HashMap::new();
        let Some(nvml) = &self.nvml else { return (out, procs) };
        let count = nvml.device_count().unwrap_or(0);
        let mut newest = self.last_seen;
        for i in 0..count {
            let Ok(d) = nvml.device_by_index(i) else { continue };
            let util = d.utilization_rates().ok();
            let mem = d.memory_info().ok();
            out.push(GpuStats {
                name: d.name().unwrap_or_else(|_| format!("GPU {i}")),
                util: util.as_ref().map(|u| u.gpu).unwrap_or(0),
                mem_util: util.as_ref().map(|u| u.memory).unwrap_or(0),
                mem_used: mem.as_ref().map(|m| m.used).unwrap_or(0),
                mem_total: mem.as_ref().map(|m| m.total).unwrap_or(0),
                temp: d.temperature(TemperatureSensor::Gpu).ok(),
                power_w: d.power_usage().ok().map(|mw| mw as f32 / 1000.0),
            });
            if let Ok(samples) = d.process_utilization_stats(self.last_seen) {
                for s in samples {
                    newest = newest.max(s.timestamp);
                    let e = procs.entry(s.pid).or_default();
                    e.0 = e.0.max(s.sm_util);
                }
            }
            for list in [d.running_compute_processes(), d.running_graphics_processes()] {
                for p in list.unwrap_or_default() {
                    if let UsedGpuMemory::Used(bytes) = p.used_gpu_memory {
                        procs.entry(p.pid).or_default().1 += bytes;
                    }
                }
            }
        }
        self.last_seen = newest;
        (out, procs)
    }
}

pub fn spawn(manager: Arc<Manager>, emit: impl Fn(SystemStats) + Send + 'static) {
    thread::Builder::new()
        .name("system-monitor".into())
        .spawn(move || {
            let mut sys = System::new();
            let mut gpu = Gpu::new();
            sys.refresh_cpu_usage();
            loop {
                thread::sleep(Duration::from_millis(1000));
                sys.refresh_cpu_usage();
                sys.refresh_memory();
                let cores = sys.cpus().len().max(1);
                let engines = manager.engine_processes();
                let pids: Vec<Pid> = engines.iter().map(|(_, _, p)| Pid::from_u32(*p)).collect();
                sys.refresh_processes_specifics(
                    ProcessesToUpdate::Some(&pids),
                    true,
                    ProcessRefreshKind::nothing().with_cpu().with_memory(),
                );
                let (gpus, gpu_procs) = gpu.sample();
                let engines = engines
                    .into_iter()
                    .map(|(session, name, pid)| {
                        let p = sys.process(Pid::from_u32(pid));
                        let g = gpu_procs.get(&pid);
                        EngineProcStats {
                            session,
                            name,
                            pid,
                            cpu: p.map(|p| p.cpu_usage() / cores as f32).unwrap_or(0.0),
                            mem_bytes: p.map(|p| p.memory()).unwrap_or(0),
                            gpu_util: g.map(|g| g.0),
                            gpu_mem: g.map(|g| g.1).filter(|&m| m > 0),
                        }
                    })
                    .collect();
                emit(SystemStats {
                    cpu: sys.global_cpu_usage(),
                    cores,
                    mem_used: sys.used_memory(),
                    mem_total: sys.total_memory(),
                    gpus,
                    gpu_error: gpu.error.clone(),
                    engines,
                });
            }
        })
        .expect("spawn monitor thread");
}
