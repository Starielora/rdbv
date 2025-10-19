#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{cell::RefCell, error::Error};
use std::rc::Rc;
use std::time::Instant;

use slint::{ModelRc, StandardListViewItem, VecModel};

slint::include_modules!();

use scopeguard::defer;

trait SlintDataSrc {
    fn get_kv(&self, cf_name: &str) -> VecModel<slint::ModelRc<StandardListViewItem>>;
    fn get_cfs(&self) -> VecModel<StandardListViewItem>;
}

struct NullData{}
impl SlintDataSrc for NullData {
    fn get_kv(&self, _cf_name: &str) -> VecModel<ModelRc<StandardListViewItem>> {
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
                Formatting::None(_) => Ok(format_ascii(val)),
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
        const PATH: &str = "temp_base";

        let cf_names = rocksdb::DB::list_cf(&opts, path)?;

        let error_if_log_file_exists = false; // should be true, but fucking rocks does not clean up itself properly
        let db = rocksdb::DB::open_cf_for_read_only(&opts, PATH, &cf_names, error_if_log_file_exists)?;

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

        let cf_handle = self.db.cf_handle(cf_name).unwrap();
        let v = self.db.get_pinned_cf(cf_handle, key)?.unwrap();
        format_val(&v, formatting)
    }

}

impl SlintDataSrc for RdbData {
    fn get_kv(&self, cf_name: &str) -> VecModel<slint::ModelRc<StandardListViewItem>> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Column family {} query time: {:?}", cf_name, duration);
        }

        let db = &self.db;

        let cf_handle = db.cf_handle(cf_name).unwrap();

        println!("{:?}", db.get_column_family_metadata_cf(cf_handle).name);

        let mut it = db.raw_iterator_cf(cf_handle);
        it.seek_to_first();

        let row_data: VecModel<slint::ModelRc<StandardListViewItem>> = VecModel::default();
        while it.valid() {
            let key = std::str::from_utf8(it.key().unwrap()).unwrap();
            let val = it.value().unwrap();

            let val_str = format_val(&val, Formatting::None(64)).unwrap();

            let items = Rc::new(VecModel::default());
            items.push(key.into());
            items.push(val_str.as_str().into());

            row_data.push(items.into());

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

fn main() -> Result<(), Box<dyn Error>> {
    let ui = AppWindow::new()?;

    let rdb_data_src: Rc<RefCell<Option<RdbData>>> = Rc::new(RefCell::new(None));

    ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(NullData{}.get_kv("")).into());

    let ui_handle = ui.as_weak();
    let rdb_data_src_clone = rdb_data_src.clone(); // huh, pls help
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
        match rdb_data_src_clone.borrow().as_ref().as_ref().unwrap().get_val(cf.as_str(), key.as_str(), formatting) {
            Ok(val) => {
                ui.set_db_value_preview(val.into());
                ui.set_status_msg(format!("Query time (with formatting): {:?}", start.elapsed()).into());
            }
            Err(e) => ui.set_status_msg(e.to_string().into()),
        }
    });

    let ui_handle = ui.as_weak();
    let rdb_data_src_clone = rdb_data_src.clone(); // huh, pls help
    ui.on_change_column_family(move |new_cf|{
        if new_cf.is_empty() {
            return;
        }
        let ui = ui_handle.unwrap();
        let start = Instant::now();
        let data = rdb_data_src_clone.borrow().as_ref().as_ref().unwrap().get_kv(new_cf.as_str());
        let duration = start.elapsed();
        ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(data).into());
        ui.set_status_msg(format!("{} CF query time: {:?}", new_cf, duration).into());
    });

    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone(); // huh, pls help
    ui.global::<DbLoader>().on_load_db(move |path| {
        let ui = ui_handle.unwrap();
        println!("{}", path.as_str());
        let mut db = rdb_data_src_handle.borrow_mut();
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
        ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(NullData{}.get_kv("")).into());
        ui.global::<ListViewAdapter>().set_list_items(Rc::new(src.get_cfs()).into());
        ui.set_page_index(1);
        ui.set_status_msg(format!("Db open time: {:?}", duration).into());
    });

    // TODO this shares most code with on_load_db
    let ui_handle = ui.as_weak();
    let rdb_data_src_handle = rdb_data_src.clone(); // huh, pls help
    ui.global::<DbLoader>().on_browse_for_db(move ||{
        let folder = rfd::FileDialog::new().set_directory("./").pick_folder();

        match folder {
            Some(path) => {
                let ui = ui_handle.unwrap();
                println!("{:?}", path);
                let mut db = rdb_data_src_handle.borrow_mut();
                let start = Instant::now();
                let db_open_result = RdbData::new(path.into_os_string().into_string().unwrap());

                if db_open_result.is_err() {
                    println!("{}", db_open_result.err().unwrap().into_string());
                    return;
                }

                let new_data_src = db_open_result.unwrap();

                let duration = start.elapsed();
                *db = Some(new_data_src);

                let src = db.as_ref().unwrap();
                ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(NullData{}.get_kv("")).into());
                ui.global::<ListViewAdapter>().set_list_items(Rc::new(src.get_cfs()).into());
                ui.set_page_index(1);
                ui.set_status_msg(format!("Db open time: {:?}", duration).into());
            },
            None => {},
        }
    });

    ui.run()?;

    Ok(())
}