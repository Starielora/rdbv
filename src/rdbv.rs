// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;
use std::ops::Add;
use std::sync::atomic::AtomicBool;
use std::error::Error;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, StandardListViewItem, VecModel};

slint::include_modules!();

use scopeguard::defer;

use crate::worker::WorkerThread;

mod worker;

use std::panic;
use windows_sys::{core::*, Win32::UI::Shell::*, Win32::UI::WindowsAndMessaging::*};

enum Formatting {
    None(usize),
    Json(),
    Hex(usize)
}

struct RdbData{
    cf_names: Vec<String>,
    db: rocksdb::DB,
}

fn format_ascii_u8(v: u8) -> char
{
    if !v.is_ascii_graphic() { '.' } else { v as char }
}

// TODO This must be slow af, pls fix
// TODO iterator magic? pls be faster? maybe separate panel would be better
// unless llvm saves me here... in any case check later
fn format_hex_ascii(val: &[u8]) -> String {

    let format_chunk = |chunk: &[u8], hex_part: &mut String, ascii_part: &mut String, result: &mut String| {
        ascii_part.clear();
        hex_part.clear();
        for c in chunk {
            hex_part.push_str(format!(" {:02X}", *c).as_str());
            ascii_part.push(format_ascii_u8(*c));
        }
        result.push_str(format!("{} |{}\n", hex_part, ascii_part).as_str());
    };

    // TODO there's gotta be a better way...
    let format_remainder = |chunk: &[u8], hex_part: &mut String, ascii_part: &mut String, result: &mut String|  {
        ascii_part.clear();
        hex_part.clear();
        for c in chunk {
            hex_part.push_str(format!(" {:02X}", *c).as_str());
            ascii_part.push(format_ascii_u8(*c));
        }
        for _ in 0..(16-chunk.len()) {
            hex_part.push_str("   ");
            ascii_part.push_str(" ");
        }
        result.push_str(format!("{} |{}\n", hex_part, ascii_part).as_str());
    };

    let mut result = String::new();
    let mut ascii_part = String::new();
    let mut hex_part = String::new();

    let (val_chunks, remainder) = val.as_chunks::<16>();
    for chunk in val_chunks {
        format_chunk(chunk, &mut hex_part, &mut ascii_part, &mut result);
    }

    if !remainder.is_empty() {
        format_remainder(remainder, &mut hex_part, &mut ascii_part, &mut result);
    }

    result
}

fn format_val(val: &[u8], formatting: Formatting) -> Result<String, Box<dyn Error>>
{
    let (val, was_cut) = match formatting {
        Formatting::None(max_chars) | Formatting::Hex(max_chars) => {
            let range = usize::min(max_chars, val.len());
            let val = val.get(..range).ok_or(format!("Invalid subslice range. Tried to get [0..{}], but slice has len {}", range, val.len()).to_string())?;
            let was_cut = val.len() > max_chars;
            (val, was_cut)
        },
        Formatting::Json() => (val, false), // assume json formatting always parses full json
    };

    match std::str::from_utf8(val) {
        Ok(v) => {
            return match formatting {
                Formatting::None(_) => {
                    let mut result = String::from(v);
                    if was_cut {
                        result.push('…');
                    }
                    Ok(result)
                },
                Formatting::Json() => {
                    return match formatjson::format_json(v) {
                        Ok(v) => Ok(v),
                        Err(_err) => Err("Nah bro, can't format as json")?,
                    }
                },
                Formatting::Hex(_) => Ok(format_hex_ascii(val)),
            }

        },
        Err(_) => { // treat as a blob, bro

            let mut result = match formatting {
                Formatting::None(_) => Ok(String::from_utf8_lossy(val).into_owned()),
                Formatting::Json() => Err("Nah bro, can't format as json"),
                Formatting::Hex(_) => Ok(format_hex_ascii(val)),
            }?;

            if was_cut {
                result.push('…');
            }

            Ok(result)
        },
    }
}

impl RdbData {
    pub fn new(path: String) -> Result<Self, rocksdb::Error> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Duration: {:?}", duration);
        }
        let opts = rocksdb::Options::default();

        let cf_names = rocksdb::DB::list_cf(&opts, &path)?;

        let error_if_log_file_exists = false; // should be true, but fucking rocks does not clean up itself properly
        let db = rocksdb::DB::open_cf_for_read_only(&opts, &path, &cf_names, error_if_log_file_exists)?;

        Ok(Self {
            cf_names,
            db,
        })
    }

    pub fn get_val(&self, cf_name: &str, key: &str, formatting: Formatting) -> Result<String, Box<dyn Error>> {
        let v = self.get_raw_val(cf_name, key)?;
        format_val(&v, formatting)
    }

    fn get_cf_handle(&self, cf_name: &str) -> Result<&rocksdb::ColumnFamily, Box<dyn Error>> {
        Ok(self.db.cf_handle(cf_name).ok_or(format!("Failed to get handle for cf {}", cf_name))?) // Fails only if UI passess different string - should I expect validity and crash app otherwise?
    }

    pub fn get_raw_val(&self, cf_name: &str, key: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Value query time for {}: {:?}", key, duration);
        }

        let cf_handle = self.get_cf_handle(cf_name)?;
        Ok(self.db.get_pinned_cf(cf_handle, key)?.ok_or(format!("No value found for key {:?}", key))?.to_vec())
    }

    pub fn get_keys(&self, cf_name: &str, progress_report: Box<dyn Fn(f32)>, set_progress_indeterminate: Box<dyn Fn()>, cancel: &AtomicBool) -> Result<Vec<String>, Box<dyn Error>> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Column family {} keys query time: {:?}", cf_name, duration);
        }

        let db = &self.db;

        let cf_handle = self.get_cf_handle(cf_name)?;

        let est_keys_num = if let Ok(Some(est_keys_num)) = db.property_int_value_cf(cf_handle, "rocksdb.estimate-num-keys") {
            est_keys_num
        } else {
            set_progress_indeterminate();
            0
        };

        let mut opts = rocksdb::ReadOptions::default();
        opts.set_async_io(true);
        opts.set_pin_data(true);
        opts.fill_cache(false);
        opts.set_allow_unprepared_value(true);
        let mut it = db.raw_iterator_cf_opt(cf_handle, opts);
        it.seek_to_first();

        let mut keys = Vec::new();

        let mut actual_keys_num = 0;
        while it.valid() {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            let t = Instant::now();
            let key = std::str::from_utf8(it.key().expect("Database invalid iterator access."))?;

            keys.push(key.to_string());

            println!("Query time {:?}. Key: {}", t.elapsed(), key);
            it.next();
            actual_keys_num += 1;
            progress_report(actual_keys_num as f32 / est_keys_num as f32);
        }

        Ok(keys)
    }
}

// TODO customize further, check upgrade_in_event_loop result
macro_rules! toggle_progress_bar {
    ($ui_handle:expr, $op_name:expr, $is_indeterminate:ident, $show:ident) => {{
        let ui_handle = $ui_handle.clone();
        move |_cancel| {
            let op_name = $op_name.clone();
            let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                ui.set_progress_is_indeterminate($is_indeterminate);
                ui.set_work_in_progress_name(op_name.into());
                ui.set_work_in_progress($show);
            });
        }
    }};
}

macro_rules! show_indeterminate_progress_bar {
    ($ui_handle:expr, $op_name:expr) => {
        toggle_progress_bar!($ui_handle, $op_name, true, true)
    };
}

macro_rules! hide_progress_bar {
    ($ui_handle:expr) => {
        toggle_progress_bar!($ui_handle, "", false, false)
    };
}

fn db_value_preview_handler(ui: &AppWindow, rdb: &RdbData,cf: &str, key: &str, formatting: Formatting, full_view: bool) {
    debug_assert!(!cf.is_empty());
    debug_assert!(!key.is_empty());

    let start = Instant::now();

    if full_view {
        ui.set_db_full_value_preview("".into());
    }
    else {
        ui.set_db_value_preview("".into());
    }

    match rdb.get_val(cf, key, formatting) {
        Ok(val) => {
            if full_view {
                ui.set_db_full_value_preview(val.into());
            } else {
                ui.set_db_value_preview(val.into());
            }
            ui.set_status_msg(format!("Query time (with formatting): {:?}", start.elapsed()).into());
        }
        Err(e) => ui.set_status_msg(e.to_string().into()),
    }
}

fn open_db(path: String, ui_handle: &slint::Weak<AppWindow>, rdb_handle: &Arc<Mutex<Option<RdbData>>>, worker: &WorkerThread) {
    println!("{:?}", path.as_str());

    let mut tasks: Vec<Box<dyn Fn(&AtomicBool) + Send>> = Vec::new();
    let progress_bar_title = format!("Opening {:?}", path);
    tasks.push(Box::new(show_indeterminate_progress_bar!(ui_handle, progress_bar_title)));

    let ui_clone = ui_handle.clone();
    let rdb_handle = rdb_handle.clone();
    tasks.push(Box::new(
        move |_cancel| {
            let start = Instant::now();
            match RdbData::new(path.to_string()) {
                Ok(rdb) => {
                    let duration = start.elapsed();
                    let cf_names = rdb.cf_names.clone();

                    {
                        let mut rdb_guard = rdb_handle.lock().expect("rdb mutex poisoned. Worker thread panicked during db access?");
                        *rdb_guard = Some(rdb);
                    }

                    let path_clone = path.clone();
                    let _ = ui_clone.upgrade_in_event_loop(move |handle|{

                        let cf_data: VecModel<StandardListViewItem> = VecModel::default();
                        for cf in cf_names.iter() {
                            cf_data.push(cf.as_str().into());
                        }

                        handle.global::<TableViewPageAdapter>().set_row_data(Rc::new(VecModel::default()).into());
                        handle.global::<ListViewAdapter>().set_list_items(Rc::new(cf_data).into());
                        handle.set_status_msg(format!("Db open time: {:?}", duration).into());
                        handle.set_loaded_db_path(path_clone.into());
                    }).map_err(print_stderr);
                },
                Err(err) => {
                    // TODO pass this error msg to UI
                    println!("{}", err.into_string());
                },
            }

        }
    ));

    tasks.push(Box::new(hide_progress_bar!(ui_handle)));

    worker.push_tasks(tasks);
}

fn get_formatting(ui_formatting: &str, chars_num: usize) -> Formatting {
    match ui_formatting {
        "None" => Formatting::None(chars_num),
        "json" => Formatting::Json(),
        "hex" => Formatting::Hex(chars_num),
        _ => todo!("Unknown formatting value from UI."),
    }
}

fn print_stderr(err: slint::EventLoopError) {
    eprintln!("{:?}", err);
}

macro_rules! lock_db {
    ($handle:ident, $guard:ident, $rdb:ident) => {
        let $guard = $handle.lock().expect("rdb mutex poisoned. Worker thread panicked during db access?");
        let $rdb = $guard.as_ref().expect("Value preview handler called without database loaded");
    };
}

fn main() -> Result<(), Box<dyn Error>> {

    let ui = AppWindow::new()?;
    let critical_error_occurred: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(Option::None));

    panic::set_hook(Box::new({
        let ui = ui.as_weak();
        let critical_error_occured = critical_error_occurred.clone();
        move |panic_info| {
        let msg = format!("{panic_info}");
        unsafe {

            {
                // if this lock fails then no msg will be printed and app exit code will be 0. 
                // Should I fix this with an AtomicBool or sth, or is this failure impossible?
                if let Ok(mut critical_error_guard) = critical_error_occured.lock() {
                    *critical_error_guard = Some(msg.clone());
                }
            }

            let _ = ui.upgrade_in_event_loop(|ui| {
                ui.window().dispatch_event(slint::platform::WindowEvent::CloseRequested);
            });

            ShellMessageBoxA(
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                msg.as_ptr(),
                s!("Critical error"),
                MB_ICONERROR,
            );
        }
    }}));


    let notice_text = include_bytes!("../NOTICE");
    ui.set_notice_text(std::str::from_utf8(notice_text)?.into());
    ui.set_window_name(format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")).into());

    ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(VecModel::default()).into());
    ui.global::<ListViewAdapter>().set_list_items(Rc::new(VecModel::default()).into());

    let worker = Arc::new(worker::WorkerThread::new());
    let rdb_data_src: Arc<Mutex<Option<RdbData>>> = Arc::new(Mutex::new(None));

    ui.global::<DbLoader>().on_load_db({
        let ui_handle = ui.as_weak();
        let rdb_handle = rdb_data_src.clone();
        let worker = worker.clone();
        move |path| {
            open_db(path.to_string(), &ui_handle, &rdb_handle, &worker);
        }
    });

    ui.global::<DbLoader>().on_browse_for_db({
        let ui_handle = ui.as_weak();
        let rdb_handle = rdb_data_src.clone();
        let worker = worker.clone();

        move ||{
            let folder = rfd::FileDialog::new().set_directory("./").pick_folder();

            if let Some(path) = folder {
                let path = path.into_os_string().into_string().expect("Got string which is inconvertible to UTF-8 from folder picker.");
                open_db(path, &ui_handle, &rdb_handle, &worker);
            }
        }
    });

    ui.on_change_db_value_preview({
        let ui_handle = ui.as_weak();
        let rdb_handle = rdb_data_src.clone();

        move |cf, key, ui_formatting| {

            if cf.is_empty() || key.is_empty() || ui_formatting.is_empty() {
                return;
            }

            if let Some(ui) = ui_handle.upgrade() {
                let formatting = get_formatting(ui_formatting.as_str(), 2048);

                lock_db!(rdb_handle, guard, rdb);
                db_value_preview_handler(&ui, rdb, cf.as_str(), key.as_str(), formatting, false);
            }
        }
    });

    // TODO shares most code with preview
    ui.on_change_db_value_full_view({
        let ui_handle = ui.as_weak();
        let rdb_handle = rdb_data_src.clone();

        move |cf, key, ui_formatting|{

            if cf.is_empty() || key.is_empty() || ui_formatting.is_empty() {
                return;
            }

            if let Some(ui) = ui_handle.upgrade() {
                lock_db!(rdb_handle, guard, rdb);

                let formatting = get_formatting(ui_formatting.as_str(), usize::MAX);

                db_value_preview_handler(&ui, rdb, cf.as_str(), key.as_str(), formatting, true);
            }
        }
    });

    ui.on_change_column_family({
        let ui_handle = ui.as_weak();
        let rdb_handle = rdb_data_src.clone();
        let worker = worker.clone();
        move |cf, query_values|{
            if cf.is_empty() {
                return;
            }

            worker.cancel_currently_scheduled_work();

            let mut tasks: Vec<Box<dyn Fn(&AtomicBool) + Send>> = Vec::new();

            tasks.push(
                Box::new({
                let ui = ui_handle.clone();
                let rdb_handle = rdb_handle.clone();
                move |cancel| {

                    let ui_handle = ui.clone();
                    let rdb_handle = rdb_handle.clone();
                    let cf = cf.clone();
                    let result = move || -> Result<(), Box<dyn Error>> {
                        let progress_bar_title = format!("Loading keys of {}", cf.as_str());
                        let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                            ui.set_progress_is_indeterminate(false);
                            ui.set_work_in_progress_name(progress_bar_title.into());
                            ui.set_work_in_progress(true);
                        }).map_err(print_stderr);

                        let progress_report = Box::new({
                            let ui = ui_handle.clone();
                            move |progress: f32| {
                                let _ = ui.upgrade_in_event_loop(move |handle|{
                                    handle.set_work_progress(progress);
                                }).map_err(print_stderr);
                            }
                        });

                        let set_progres_indeterminate = Box::new({
                            let ui = ui_handle.clone();
                            move || {
                                let _ = ui.upgrade_in_event_loop(move |handle|{
                                    handle.set_progress_is_indeterminate(true);
                                }).map_err(print_stderr);
                            }
                        });

                        let start = Instant::now();
                        let keys = {
                            lock_db!(rdb_handle, guard, rdb);
                            rdb.get_keys(cf.as_str(), progress_report, set_progres_indeterminate, cancel)?
                        };
                        let duration = start.elapsed();

                        // TODO could preallocate this vec when getting keys instead of copying
                        // For now perf is sufficient
                        let mut row_data: Vec<(String, String)> = Vec::new();
                        for k in keys.iter() {
                            row_data.push((k.to_string(), "".to_string()));
                        }

                        let _ = ui_handle.upgrade_in_event_loop({
                            let cf = cf.clone();
                            move |handle|{
                                let ui_row_data: VecModel<slint::ModelRc<StandardListViewItem>> = VecModel::default();

                                // VecModel is not Send and cannot be prepared on second thread.
                                for (k, v) in row_data.iter() {
                                    let items = Rc::new(VecModel::default());
                                    items.push(k.as_str().into());
                                    items.push(v.as_str().into());
                                    ui_row_data.push(items.into());
                                }

                                handle.global::<TableViewPageAdapter>().set_row_data(Rc::new(ui_row_data).into());
                                handle.set_status_msg(format!("{} CF keys query time: {:?}", cf, duration).into());
                                handle.set_work_in_progress(false);
                            }
                        }).map_err(print_stderr);

                        if query_values {

                            let ui = ui_handle.clone();
                            let _ = ui.upgrade_in_event_loop(move |handle|{
                                handle.set_work_progress(0.0);
                                handle.set_work_in_progress(true);
                            }).map_err(print_stderr);

                            let cf = cf.clone();
                            let mut total_values_query_time = Duration::new(0, 0);
                            for (i, key) in keys.iter().enumerate() {
                                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                    break;
                                }

                                let progress_bar_title = format!("Loading {}", key);
                                let _ = ui.upgrade_in_event_loop(move |ui| {
                                    ui.set_work_in_progress_name(progress_bar_title.into());
                                }).map_err(print_stderr);

                                let start = Instant::now();
                                let value = {
                                    lock_db!(rdb_handle, guard, rdb);
                                    rdb.get_val(cf.as_ref(), key, Formatting::None(2048))?.lines().nth(0).ok_or("Failed to extract first line from value to display in UI")?.to_string()
                                };

                                total_values_query_time = total_values_query_time.add(start.elapsed());

                                let _ = ui.upgrade_in_event_loop({
                                    let progress = i as f32 / keys.len() as f32;
                                    move |handle|{
                                        // if the expect fails it means something must've changed the TableView inbetween
                                        // in current design, this operation is not holding table view exclusively, which leaves it open for such bug
                                        // TODO in such ocurrence display an explicit dialog, instead of panicking the app?
                                        handle.global::<TableViewPageAdapter>().get_row_data().row_data_tracked(i).expect(format!("Failed to get {} row of KV Table", i).as_str()).set_row_data(1, value.as_str().into());
                                        handle.set_work_progress(progress);
                                    }
                                }).map_err(print_stderr);
                            }

                            let _ = ui.upgrade_in_event_loop(move |ui| {
                                ui.set_status_msg(format!("{} values query time: {:?}", cf, total_values_query_time).into());
                                ui.set_work_in_progress(false);
                            }).map_err(print_stderr);
                        }
                        Ok(())
                    };

                    match result() {
                        Ok(_) => {},
                        Err(err) => {
                            let msg = format!("{}", err.to_string());
                            let _ = ui.upgrade_in_event_loop(move |ui| {
                                ui.set_status_msg(msg.as_str().into());
                            });
                        },
                    }

            }}));

            worker.push_tasks(tasks);
        }
    });

    ui.global::<DbLoader>().on_export_value_to_file({
        let ui_handle = ui.as_weak();
        let rdb_handle = rdb_data_src.clone();

        move |cf, key|{
            let file = rfd::FileDialog::new().set_directory("./").save_file();

            let start = Instant::now();
            defer!{
                let duration = start.elapsed();
                println!("Duration: {:?}", duration);
            }

            if let Some(ui) = ui_handle.upgrade() {
                lock_db!(rdb_handle, guard, rdb);

                if let Some(file) = file {
                    let _ = || -> Result<(), Box<dyn Error>> {
                        let buffer = rdb.get_raw_val(cf.as_str(), key.as_str())?;
                        std::fs::File::create_new(file)?.write(&buffer.as_slice())?;
                        Ok(())
                    }().map_err(|e| {
                        ui.set_status_msg(e.to_string().into());
                    });
                }
            }
        }
    });

    ui.run()?;

    match critical_error_occurred.lock() {
        Ok(mut msg_opt) => {
            if let Some(msg) = (*msg_opt).take() {
                Err(msg)?
            }
        },
        Err(mut err) => {
            // best effort at retrieving msg?
            // does it even make sense? it would mean that panic handler panicked while holding this mtx
            if let Some(msg) = (*err.get_mut()).take() {
                Err(msg)?
            }
        },
    };

    Ok(())
}