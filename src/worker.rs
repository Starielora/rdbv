use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Condvar};
use std::option::Option;

pub struct WorkerThread {
    thread_handle: Option<std::thread::JoinHandle<()>>,
    wakeup: Arc<(Mutex<bool>, Condvar)>,
    tasks: Arc<Mutex<Vec<Box<dyn Fn(&AtomicBool) + Send>>>>,
    cancel: Arc<AtomicBool>,
}

impl WorkerThread {
    pub fn new() -> WorkerThread {

        let wakeup = Arc::new((Mutex::new(false), Condvar::new()));
        let tasks: Arc<Mutex<Vec<Box<dyn Fn(&AtomicBool) + Send>>>> = Arc::new(Mutex::new(Vec::new()));
        let cancel = Arc::new(AtomicBool::new(false));

        let start_barrier = Arc::new(std::sync::Barrier::new(1));

        let thread = std::thread::spawn({
                let wakeup = wakeup.clone();
                let tasks = tasks.clone();
                let cancel = cancel.clone();
                let start_barrier = start_barrier.clone();
                move || {

                let (wakeup_mtx, cvar) = &*wakeup;

                'work_loop: loop {
                    start_barrier.wait();
                    println!("Waiting for werk");
                    {
                        let wakeup_guard = match wakeup_mtx.lock() {
                            Ok(guard) => guard,
                            Err(_) => break 'work_loop,
                        };
                        let shutdown = match cvar.wait_while(wakeup_guard, |shutdown| {
                            !*shutdown && match tasks.lock() {
                                Ok(tasks) => tasks.is_empty(),
                                Err(_) => {
                                    // this worker thread has poisoned a mutex, which means main thread panicked while inserting tasks.
                                    // A very unlikely scenario, and it would also crash the whole app regardless, which is handled in panic handler.
                                    *shutdown = true;
                                    false
                                },
                            }
                        })
                        {
                            Ok(guard) => guard,
                            Err(_) => break 'work_loop
                        };

                        if *shutdown {
                            println!("Shutdown requested. Bye");
                            break 'work_loop;
                        }
                    }

                    println!("Doing werk");

                    cancel.store(false, std::sync::atomic::Ordering::Relaxed);

                    let mut tasks_guard = match tasks.lock() {
                        Ok(guard) => guard,
                        Err(_) => break 'work_loop,
                    };

                    let tasks = &mut *tasks_guard;

                    for task in tasks.iter() {
                        let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed);
                        if cancelled {
                            break 'work_loop;
                        }
                        else {
                            task(&*cancel);
                        }
                    }

                    tasks.clear();

                }
            }});

        start_barrier.wait();

        Self {
            thread_handle: Some(thread),
            wakeup,
            tasks,
            cancel,
        }
    }

    pub fn push_tasks(&self, mut user_tasks: Vec<Box<dyn Fn(&AtomicBool) + Send>>) {
        let mut tasks_guard = self.tasks.lock().expect("Poisoned mtx");
        let tasks = &mut *tasks_guard;
        tasks.append(&mut user_tasks);

        self.wakeup.1.notify_one();
    }

    pub fn cancel_currently_scheduled_work(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl std::ops::Drop for WorkerThread {
    fn drop(&mut self) {

        self.cancel_currently_scheduled_work();

        let (shutdown_mtx, cvar) = &*self.wakeup;

        {
            let mut shutdown_guard = shutdown_mtx.lock().expect("Poisoned mtx");
            *shutdown_guard = true;
        }

        cvar.notify_one();

        if let Some(thread) = self.thread_handle.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn start_stop_doesnt_hang() {
        let werker = WorkerThread::new();
    }

    #[test]
    fn push_work() {
        let werker = WorkerThread::new();
        let mut tasks: Vec<Box<dyn Fn(&AtomicBool) + Send>> = Vec::new();

        let mut w1 = Arc::new(Mutex::new(false));
        let mut w1_cvar = Arc::new(Condvar::new());

        let w1_clone = w1.clone(); 
        let w1_cvar_clone = w1_cvar.clone();
        tasks.push(Box::new(move |_cancel| {
            *w1_clone.lock().unwrap() = true;
            w1_cvar_clone.notify_one();
        }));

        let mut w2 = Arc::new(Mutex::new(false));
        let mut w2_cvar = Arc::new(Condvar::new());

        let w2_clone = w2.clone(); 
        let w2_cvar_clone = w2_cvar.clone();
        tasks.push(Box::new(move |_cancel| {
            *w2_clone.lock().unwrap() = true;
            w2_cvar_clone.notify_one();
        }));

        werker.push_tasks(tasks);

        let val = w1_cvar.wait_while(w1.lock().unwrap(), |isset| { !*isset }).unwrap();
        assert_eq!(*val, true);

        let val = w2_cvar.wait_while(w2.lock().unwrap(), |isset| { !*isset }).unwrap();
        assert_eq!(*val, true);
    }

    #[test]
    fn cancel_work() {
        let werker = WorkerThread::new();
        let mut tasks: Vec<Box<dyn Fn(&AtomicBool) + Send>> = Vec::new();

        let mut w1_main_thread = Arc::new((Mutex::new(false), Condvar::new()));
        let mut w1_work_thread = Arc::new((Mutex::new(()), Condvar::new()));

        let w1_main_thread_clone = w1_main_thread.clone(); 
        let w1_work_thread_clone = w1_work_thread.clone();

        tasks.push(Box::new(move |_cancel| {
            *w1_main_thread_clone.0.lock().unwrap() = true;
            w1_main_thread_clone.1.notify_one();

            let _unused = w1_work_thread_clone.1.wait(w1_main_thread_clone.0.lock().unwrap()).unwrap();
        }));

        tasks.push(Box::new(move |_cancel| {
            panic!("This task should not have executed");
        }));

        werker.push_tasks(tasks);

        let val = w1_main_thread.1.wait_while(w1_main_thread.0.lock().unwrap(), |isset| { !*isset }).unwrap();
        assert_eq!(*val, true);

        werker.cancel_currently_scheduled_work();

        w1_work_thread.1.notify_one();
    }
}