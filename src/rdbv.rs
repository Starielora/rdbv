// #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write;
use std::ops::Add;
use std::sync::atomic::AtomicBool;
use std::{cell::RefCell, error::Error};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{Model, ModelRc, StandardListViewItem, VecModel};

slint::include_modules!();

use scopeguard::defer;

use crate::worker::WorkerThread;

mod worker;

trait SlintDataSrc {
    fn get_kv(&self, cf_name: &str, query_values: bool) -> VecModel<slint::ModelRc<StandardListViewItem>>;
    fn get_cfs(&self) -> VecModel<StandardListViewItem>;
}

struct NullData{}
impl SlintDataSrc for NullData {
    fn get_kv(&self, _cf_name: &str, _query_values: bool) -> VecModel<ModelRc<StandardListViewItem>> {
        let row_data: VecModel<slint::ModelRc<StandardListViewItem>> = VecModel::default();
        row_data
    }

    fn get_cfs(&self) -> VecModel<StandardListViewItem> {
        let cf_data: VecModel<StandardListViewItem> = VecModel::default();
        cf_data
    }
}

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
    if !v.is_ascii_graphic() { '.' } else { char::from_u32((v).into()).unwrap() }
}

fn format_ascii(val: &[u8]) -> String {

    let mut result = String::new();
    for c in val {
        result.push(format_ascii_u8(*c));
    }
    result
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
        Formatting::None(max_chars) => (val.get(..usize::min(max_chars, val.len())).unwrap(), val.len() > max_chars),
        Formatting::Json() => (val, false), // assume json formatting always parses full json
        Formatting::Hex(max_chars) => (val.get(..usize::min(max_chars, val.len())).unwrap(), val.len() > max_chars),
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
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Value query time: {:?}", duration);
        }

        println!("Query: {:?}", key);
        let cf_handle = self.db.cf_handle(cf_name).unwrap();
        let v = self.db.get_pinned_cf(cf_handle, key)?;
        if v.is_none() {
            Err(format!("Failed to get pinned value for key {:?}", key))?
        }
        let v = v.unwrap();
        format_val(&v, formatting)
        // Ok(String::from_utf8_lossy(&v).to_string())
    }

    pub fn get_raw_val(&self, cf_name: &str, key: &str) -> Result<Vec<u8>, Box<dyn Error>> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Value query time: {:?}", duration);
        }

        let cf_handle = self.db.cf_handle(cf_name).unwrap();
        let v = self.db.get_pinned_cf(cf_handle, key)?;
        if v.is_none() {
            Err(format!("Failed to get pinned value for key {:?}", key))?
        }
        Ok(v.unwrap().to_vec())
    }

    pub fn get_keys(&self, cf_name: &str, progress_report: Box<dyn Fn(f32)>, cancel: &AtomicBool) -> Vec<String> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Column family {} keys query time: {:?}", cf_name, duration);
        }

        let db = &self.db;

        let cf_handle = db.cf_handle(cf_name).unwrap();

        let mut opts = rocksdb::ReadOptions::default();
        opts.set_async_io(true);
        opts.set_pin_data(true);
        opts.fill_cache(false);
        opts.set_allow_unprepared_value(true);
        let mut it = db.raw_iterator_cf_opt(cf_handle, opts);
        it.seek_to_first();

        let mut keys = Vec::new();

        let est_keys_num = db.property_int_value_cf(cf_handle, "rocksdb.estimate-num-keys").unwrap().unwrap();
        let mut actual_keys_num = 0;
        while it.valid() {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            let t = Instant::now();
            let key = std::str::from_utf8(it.key().unwrap()).unwrap();

            keys.push(key.to_string());

            println!("Query time {:?}. Key: {}", t.elapsed(), key);
            it.next();
            actual_keys_num += 1;
            progress_report(actual_keys_num as f32 / est_keys_num as f32);
        }

        println!("keys: {}; actual: {}", est_keys_num, actual_keys_num);

        keys
    }

    pub fn get_value(&self, cf_name: &str, key: &str) -> Vec<u8> {
        // TODO unwrap
        let cf_handle = &self.db.cf_handle(cf_name).unwrap();
        let val = &self.db.get_cf(cf_handle, key).unwrap().unwrap();
        // TODO legit clone?
        val.clone()
    }

    pub fn get_kv_raw(&self, cf_name: &str, query_values: bool, progress_report: Box<dyn Fn(f32)>, cancel: &AtomicBool) -> Vec<(String, String)> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Column family {} query time: {:?}", cf_name, duration);
        }

        let db = &self.db;

        let cf_handle = db.cf_handle(cf_name).unwrap();

        println!("{:?}", db.get_column_family_metadata_cf(cf_handle).name);

        let mut opts = rocksdb::ReadOptions::default();
        opts.set_async_io(true);
        opts.set_pin_data(true);
        opts.fill_cache(false);
        opts.set_allow_unprepared_value(true);
        let mut it = db.raw_iterator_cf_opt(cf_handle, opts);
        it.seek_to_first();

        let mut row_data = Vec::new();

        let est_keys = db.property_int_value_cf(cf_handle, "rocksdb.estimate-num-keys").unwrap().unwrap();
        let mut actual_keys_num = 0;
        while it.valid() {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                break;
            }
            let t = Instant::now();
            let key = std::str::from_utf8(it.key().unwrap()).unwrap();
            let mut final_val = String::from("");

            // TODO possibly different loop variant to not check each iteration, although branch predictor should handle it
            if query_values {
                it.prepare_value();
                let val = it.value().unwrap();
                let val_str = format_val(&val, Formatting::None(64)).unwrap();
                final_val = val_str;
            } else {
                final_val = String::from("");
            }

            row_data.push((key.to_string(), final_val));

            println!("Query time {:?}. Key: {}", t.elapsed(), key);
            it.next();
            actual_keys_num += 1;
            progress_report(actual_keys_num as f32 / est_keys as f32);
        }

        println!("keys: {}; actual: {}", est_keys, actual_keys_num);

        row_data

    }

    pub fn get_cfs_raw(&self) -> &Vec<String> {
        &self.cf_names
    }
}

impl SlintDataSrc for RdbData {
    fn get_kv(&self, cf_name: &str, query_values: bool) -> VecModel<slint::ModelRc<StandardListViewItem>> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Column family {} query time: {:?}", cf_name, duration);
        }

        let db = &self.db;

        let cf_handle = db.cf_handle(cf_name).unwrap();

        println!("{:?}", db.get_column_family_metadata_cf(cf_handle).name);

        let mut opts = rocksdb::ReadOptions::default();
        opts.set_async_io(true);
        opts.set_pin_data(true);
        opts.fill_cache(false);
        opts.set_allow_unprepared_value(true);
        let mut it = db.raw_iterator_cf_opt(cf_handle, opts);
        it.seek_to_first();

        let row_data: VecModel<slint::ModelRc<StandardListViewItem>> = VecModel::default();
        while it.valid() {
            let t = Instant::now();
            let key = std::str::from_utf8(it.key().unwrap()).unwrap();
            let items = Rc::new(VecModel::default());
            items.push(key.into());

            // TODO possibly different loop variant to not check each iteration, although branch predictor should handle it
            if query_values {
                it.prepare_value();
                let val = it.value().unwrap();
                let val_str = format_val(&val, Formatting::None(64)).unwrap();
                items.push(val_str.as_str().into());
            } else {
                items.push("".into());
            }

            row_data.push(items.into());

            println!("Query time {:?}. Key: {}", t.elapsed(), key);
            it.next();
        }

        row_data
    }

    fn get_cfs(&self) -> VecModel<StandardListViewItem> {
        let cf_data: VecModel<StandardListViewItem> = VecModel::default();

        for cf in &self.cf_names {
            cf_data.push(cf.as_str().into());
        }

        cf_data
    }
}

macro_rules! toggle_progress_bar {
    ($ui_handle:expr, $op_name:expr, $is_indeterminate:ident, $show:ident) => {{
        let ui_handle = $ui_handle.clone();
        move |cancel| {
            let op_name = $op_name.clone();
            let _ = ui_handle.upgrade_in_event_loop(move |ui| {
                ui.set_progress_is_indeterminate($is_indeterminate);
                ui.set_work_in_progress_name(op_name.into());
                ui.set_work_in_progress($show);
            });
        }
    }};
}

macro_rules! show_progress_bar {
    ($ui_handle:expr, $op_name:expr) => {
        toggle_progress_bar!($ui_handle, $op_name, false, true)
    };
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

fn main() -> Result<(), Box<dyn Error>> {

    let ui = AppWindow::new()?;

    let werker = Arc::new(worker::WorkerThread::new());
    let rdb_data_src: Arc<Mutex<Option<RdbData>>> = Arc::new(Mutex::new(None));

    ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(NullData{}.get_kv("", false)).into());
    ui.global::<ListViewAdapter>().set_list_items(Rc::new(NullData{}.get_cfs()).into());

    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone();
    ui.on_change_db_value_preview(move |cf, key, ui_formatting| {
        if cf.is_empty() || key.is_empty() || ui_formatting.is_empty() {
            return;
        }

        let ui = ui_handle.unwrap();
        let start = Instant::now();
        ui.set_db_value_preview("".into());
        // TODO fucking string contract
        let formatting = match ui_formatting.as_str() {
            "None" => Formatting::None(2048),
            "json" => Formatting::Json(),
            "hex" => Formatting::Hex(2048),
            _ => Formatting::None(2048)
        };
        match rdb_data_src_handle.lock().unwrap().as_ref().as_ref().unwrap().get_val(cf.as_str(), key.as_str(), formatting) {
            Ok(val) => {
                ui.set_db_value_preview(val.into());
                ui.set_status_msg(format!("Query time (with formatting): {:?}", start.elapsed()).into());
            }
            Err(e) => ui.set_status_msg(e.to_string().into()),
        }
    });

    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone();
    let werker_clone = werker.clone();
    ui.on_change_column_family(move |new_cf, query_values|{
        if new_cf.is_empty() {
            return;
        }

        werker_clone.cancel_currently_scheduled_work();

        let rdb_data_src_handle = rdb_data_src_handle.clone();
        let mut tasks: Vec<Box<dyn Fn(&AtomicBool) + Send>> = Vec::new();

        let progress_bar_title = format!("Loading column family {}", new_cf.as_str());
        tasks.push(Box::new(show_progress_bar!(ui_handle, progress_bar_title)));

        let ui_handle_clone = ui_handle.clone();
        tasks.push(
            Box::new(
            move |cancel| {
                let ui2 = ui_handle_clone.clone();
                let progress_report = Box::new(move |progress: f32| {
                    let _ = ui2.upgrade_in_event_loop(move |handle|{
                        handle.set_work_progress(progress);
                    }).unwrap();
                });

                let progress_bar_title = format!("Loading keys of {}", new_cf.as_str());
                let _ = ui_handle_clone.upgrade_in_event_loop(move |ui| {
                    ui.set_progress_is_indeterminate(false);
                    ui.set_work_in_progress_name(progress_bar_title.into());
                    ui.set_work_in_progress(true);
                }).unwrap();

                let start = Instant::now();
                let keys = rdb_data_src_handle.lock().unwrap().as_ref().as_ref().unwrap().get_keys(new_cf.as_str(), progress_report, cancel);
                let duration = start.elapsed();

                let mut row_data: Vec<(String, String)> = Vec::new();

                for k in keys.iter() {
                    row_data.push((k.to_string(), "".to_string()));
                }

                let new_cf_clone = new_cf.clone();
                let _ = ui_handle_clone.upgrade_in_event_loop(move |handle|{
                    let ui_row_data: VecModel<slint::ModelRc<StandardListViewItem>> = VecModel::default();

                    for (k, v) in row_data.iter() {
                        let items = Rc::new(VecModel::default());
                        items.push(k.as_str().into());
                        items.push(v.as_str().into());
                        ui_row_data.push(items.into());
                    }

                    handle.global::<TableViewPageAdapter>().set_row_data(Rc::new(ui_row_data).into());
                    handle.set_status_msg(format!("{} CF keys query time: {:?}", new_cf_clone, duration).into());
                }).unwrap();

                if query_values {

                    let ui_handle_clone = ui_handle_clone.clone();
                    let _ = ui_handle_clone.upgrade_in_event_loop(move |handle|{
                        handle.set_work_progress(0.0);
                    }).unwrap();

                    let new_cf_clone = new_cf.clone();
                    let mut total_values_query_time = Duration::new(0, 0);
                    for (i, key) in keys.iter().enumerate() {
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }

                        let progress_bar_title = format!("Loading {}", key);
                        let _ = ui_handle_clone.upgrade_in_event_loop(move |ui| {
                            ui.set_work_in_progress_name(progress_bar_title.into());
                        }).unwrap();

                        let start = Instant::now();
                        let value = rdb_data_src_handle.lock().unwrap().as_ref().as_ref().unwrap().get_value(&new_cf_clone.as_ref(), key);
                        let value = format_val(&value, Formatting::None(2048)).unwrap().lines().nth(0).unwrap().to_string();
                        total_values_query_time = total_values_query_time.add(start.elapsed());
                        let progress = i as f32 / keys.len() as f32;
                        let _ = ui_handle_clone.upgrade_in_event_loop(move |handle|{
                            let start = Instant::now();
                            handle.global::<TableViewPageAdapter>().get_row_data().row_data_tracked(i).unwrap().set_row_data(1, value.as_str().into());
                            handle.set_work_progress(progress);
                        }).unwrap();
                    }

                    let _ = ui_handle_clone.upgrade_in_event_loop(move |ui| {
                        ui.set_status_msg(format!("{} values query time: {:?}", new_cf_clone, total_values_query_time).into());
                    }).unwrap();
                }

        }));

        tasks.push(Box::new(hide_progress_bar!(ui_handle)));

        werker_clone.push_tasks(tasks);
    });

    let open_db = |path: String, ui_handle: &slint::Weak<AppWindow>, rdb_data_src_handle: &Arc<Mutex<Option<RdbData>>>, werker: &WorkerThread| {

        println!("{:?}", path.as_str());

        let mut tasks: Vec<Box<dyn Fn(&AtomicBool) + Send>> = Vec::new();
        let progress_bar_title = format!("Opening {:?}", path);
        tasks.push(Box::new(show_indeterminate_progress_bar!(ui_handle, progress_bar_title)));

        let ui_clone = ui_handle.clone();
        let rdb_data_src_handle = rdb_data_src_handle.clone();
        tasks.push(Box::new(
            move |cancel| {
                let db = &mut *rdb_data_src_handle.lock().unwrap();
                let start = Instant::now();
                let db_open_result = RdbData::new(path.to_string());

                if db_open_result.is_err() {
                    println!("{}", db_open_result.err().unwrap().into_string());
                    return;
                }

                let new_data_src = db_open_result.unwrap();
                let duration = start.elapsed();
                *db = Some(new_data_src);

                let src = db.as_ref().unwrap();
                let cf_names = src.cf_names.clone();
                let path_clone = path.clone();
                let _ = ui_clone.upgrade_in_event_loop(move |handle|{

                    let cf_data: VecModel<StandardListViewItem> = VecModel::default();
                    for cf in cf_names.iter() {
                        cf_data.push(cf.as_str().into());
                    }

                    handle.global::<TableViewPageAdapter>().set_row_data(Rc::new(NullData{}.get_kv("", false)).into());
                    handle.global::<ListViewAdapter>().set_list_items(Rc::new(cf_data).into());
                    handle.set_status_msg(format!("Db open time: {:?}", duration).into());
                    handle.set_loaded_db_path(path_clone.into());
                });
            }
        ));

        tasks.push(Box::new(hide_progress_bar!(ui_handle)));

        werker.push_tasks(tasks);
    };

    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone();
    let werker_clone = werker.clone();
    ui.global::<DbLoader>().on_load_db(move |path| {
        open_db(path.to_string(), &ui_handle, &rdb_data_src_handle, &werker_clone);
    });

    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone();
    let werker_clone = werker.clone();
    ui.global::<DbLoader>().on_browse_for_db(move ||{
        let folder = rfd::FileDialog::new().set_directory("./").pick_folder();

        match folder {
            Some(path) => {
                let path = path.into_os_string().into_string().unwrap();
                open_db(path, &ui_handle, &rdb_data_src_handle, &werker_clone);
            },
            None => {},
        }
    });

    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone();
    // TODO shares most code with preview
    ui.on_change_db_value_full_view(move |cf, key, ui_formatting|{
        if cf.is_empty() || key.is_empty() || ui_formatting.is_empty() {
            return;
        }

        let ui = ui_handle.unwrap();
        let start = Instant::now();
        ui.set_db_full_value_preview("".into());
        // TODO fucking string contract
        let formatting = match ui_formatting.as_str() {
            "None" => Formatting::None(usize::MAX),
            "json" => Formatting::Json(),
            "hex" => Formatting::Hex(usize::MAX),
            _ => Formatting::None(usize::MAX)
        };
        match rdb_data_src_handle.lock().unwrap().as_ref().as_ref().unwrap().get_val(cf.as_str(), key.as_str(), formatting) {
            Ok(val) => {
                ui.set_db_full_value_preview(val.into());
                ui.set_status_msg(format!("Query time (with formatting): {:?}", start.elapsed()).into());
            }
            Err(e) => ui.set_status_msg(e.to_string().into()),
        }
    });

    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone();
    ui.global::<DbLoader>().on_export_value_to_file(move |cf, key|{
        let file = rfd::FileDialog::new().set_directory("./").save_file();

        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Duration: {:?}", duration);
        }

        let ui = ui_handle.unwrap();
        // TODO wtf is this match shit, fix
        match file {
            Some(path) => {
                match rdb_data_src_handle.lock().unwrap().as_ref().as_ref().unwrap().get_raw_val(cf.as_str(), key.as_str()) {
                    Ok(buffer) => {
                        match std::fs::File::create_new(path) {
                            Ok(mut file) => {
                                match file.write(&buffer.as_slice()) {
                                    Ok(_) => {
                                        ui.set_status_msg(format!("Write time {:?}", start.elapsed()).into());
                                    }
                                    Err(e) => ui.set_status_msg(e.to_string().into()),
                                }
                            },
                            Err(e) => ui.set_status_msg(e.to_string().into()),
                        }
                    },
                    Err(e) => ui.set_status_msg(e.to_string().into()),
                }
            },
            None => {},
        }
    });

    ui.run()?;

    Ok(())
}