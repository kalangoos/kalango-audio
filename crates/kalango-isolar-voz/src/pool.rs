//! A small fixed thread pool for the network's parallel loops.
//!
//! `run` hands out task indices from an atomic counter to the workers and the calling thread, and returns when every
//! task is done. It allocates nothing per call, so the realtime path can use it; idle workers spin briefly before
//! sleeping, which keeps the wake-up cost of the ~90 parallel regions of a 20 ms block low.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

type Task = dyn Fn(usize, usize) + Sync;

struct Job {
    task: *const Task,
    count: usize,
}

struct Shared {
    generation: AtomicU64,
    next: AtomicUsize,
    pending: AtomicUsize,
    stop: AtomicBool,
    job: UnsafeCell<Job>,
    lock: Mutex<()>,
    wake: Condvar,
}

// SAFETY: `job` is written only by the thread inside `run` while no worker reads it (every worker has finished the
// previous generation), and published to the workers by the release store of `generation`.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

/// A pool of `threads - 1` workers plus the calling thread.
pub struct Pool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
    threads: usize,
}

const SPINS: u32 = 20_000;

impl Pool {
    /// A pool running loops on `threads` threads in total (at least one: the caller).
    pub fn new(threads: usize) -> Pool {
        let threads = threads.max(1);
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            next: AtomicUsize::new(0),
            pending: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            job: UnsafeCell::new(Job { task: std::ptr::null::<fn(usize, usize)>() as *const Task, count: 0 }),
            lock: Mutex::new(()),
            wake: Condvar::new(),
        });
        let workers = (1..threads)
            .map(|worker| {
                let shared = shared.clone();
                std::thread::Builder::new()
                    .name(format!("isolar-voz-{worker}"))
                    .spawn(move || work(&shared, worker))
                    .expect("thread do isolar-voz")
            })
            .collect();
        Pool { shared, workers, threads }
    }

    /// Threads that run tasks, the caller included.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Runs `task(index, thread)` for every index in `0..count` and returns when all have finished. `thread` is in
    /// `0..threads()` and unique among the tasks running at the same time, for per-thread scratch.
    pub fn run<F: Fn(usize, usize) + Sync>(&self, count: usize, task: F) {
        if count == 0 {
            return;
        }
        if self.threads == 1 || count == 1 {
            for index in 0..count {
                task(index, 0);
            }
            return;
        }
        let shared = &*self.shared;
        let erased: &(dyn Fn(usize, usize) + Sync + '_) = &task;
        // SAFETY: the lifetime is erased only for the duration of this call: `run` waits until every worker has
        // finished with the job before it returns and `task` is dropped.
        let erased: *const Task =
            unsafe { std::mem::transmute::<&(dyn Fn(usize, usize) + Sync + '_), &'static Task>(erased) };
        unsafe {
            *shared.job.get() = Job { task: erased, count };
        }
        shared.next.store(0, Ordering::Relaxed);
        shared.pending.store(self.threads - 1, Ordering::Relaxed);
        {
            let _guard = shared.lock.lock().unwrap();
            shared.generation.fetch_add(1, Ordering::Release);
            shared.wake.notify_all();
        }
        loop {
            let index = shared.next.fetch_add(1, Ordering::Relaxed);
            if index >= count {
                break;
            }
            task(index, 0);
        }
        let mut spins = 0u32;
        while shared.pending.load(Ordering::Acquire) != 0 {
            spins += 1;
            if spins < SPINS {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
        }
    }
}

fn work(shared: &Shared, worker: usize) {
    let mut seen = 0u64;
    loop {
        let mut spins = 0u32;
        loop {
            let generation = shared.generation.load(Ordering::Acquire);
            if generation != seen {
                seen = generation;
                break;
            }
            if shared.stop.load(Ordering::Relaxed) {
                return;
            }
            spins += 1;
            if spins < SPINS {
                std::hint::spin_loop();
                continue;
            }
            let guard = shared.lock.lock().unwrap();
            if shared.generation.load(Ordering::Acquire) == seen && !shared.stop.load(Ordering::Relaxed) {
                drop(shared.wake.wait(guard).unwrap());
            }
            spins = 0;
        }
        if shared.stop.load(Ordering::Relaxed) {
            return;
        }
        // SAFETY: published by the release store of `generation` observed above; stays valid until `pending` hits 0.
        let (task, count) = unsafe {
            let job = &*shared.job.get();
            (&*job.task, job.count)
        };
        loop {
            let index = shared.next.fetch_add(1, Ordering::Relaxed);
            if index >= count {
                break;
            }
            task(index, worker);
        }
        shared.pending.fetch_sub(1, Ordering::Release);
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        {
            let _guard = self.shared.lock.lock().unwrap();
            self.shared.stop.store(true, Ordering::Relaxed);
            self.shared.wake.notify_all();
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// A raw pointer that tasks may share to write disjoint parts of one buffer.
#[derive(Clone, Copy)]
pub struct SharedMut<T>(pub *mut T);
unsafe impl<T> Send for SharedMut<T> {}
unsafe impl<T> Sync for SharedMut<T> {}

impl<T> SharedMut<T> {
    pub fn new(slice: &mut [T]) -> Self {
        SharedMut(slice.as_mut_ptr())
    }
    /// The pointer (a method, so closures capture the `Sync` wrapper rather than its field).
    pub fn ptr(&self) -> *mut T {
        self.0
    }
    /// SAFETY: the caller guarantees `[start, start + len)` is in bounds and written by no other task meanwhile.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn slice(&self, start: usize, len: usize) -> &mut [T] {
        std::slice::from_raw_parts_mut(self.0.add(start), len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_runs_once() {
        let pool = Pool::new(4);
        for round in 0..200 {
            let hits: Vec<AtomicUsize> = (0..round % 37 + 1).map(|_| AtomicUsize::new(0)).collect();
            pool.run(hits.len(), |index, thread| {
                assert!(thread < 4);
                hits[index].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1));
        }
    }
}
