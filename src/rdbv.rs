// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::error::Error;
use std::rc::Rc;
use std::time::Instant;

use slint::{Model, ModelRc, StandardListViewItem, VecModel};

slint::include_modules!();

use rocksdb::{DB};
use scopeguard::defer;

trait SlintDataSrc {
    fn get_kv(&self, cf_name: &str) -> VecModel<slint::ModelRc<StandardListViewItem>>;
    fn get_cfs(&self) -> VecModel<StandardListViewItem>;
}

struct DummyData { }

impl SlintDataSrc for DummyData {
    fn get_kv(&self, cf_name: &str) -> VecModel<ModelRc<StandardListViewItem>> {
        let row_data: VecModel<slint::ModelRc<StandardListViewItem>> = VecModel::default();

        for r in 1..101 {
            let items = Rc::new(VecModel::default());

            items.push(slint::format!("Rust key {r}").into());
            items.push(slint::format!("Rust value {r}").into());

            row_data.push(items.into());
        }

        row_data
    }

    fn get_cfs(&self) -> VecModel<StandardListViewItem> {
        let cf_data: VecModel<StandardListViewItem> = VecModel::default();

        for i in 1..5 {

            cf_data.push(slint::format!("Rust CF {i}").into());
        }

        cf_data
    }
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

struct RdbData{
    cf_names: Vec<String>,
    db: rocksdb::DB,
}

fn parse_val(val: &[u8], max_chars: usize, blob_with_hex: bool) -> String
{
    let was_cut = val.len() > max_chars;
    let val = val.get(..usize::min(max_chars, val.len())).unwrap();
    let mut parsed = match std::str::from_utf8(val) {
        Ok(v) => v.to_string(),
        Err(_) => { // treat as a blob, bro

            let mut result = String::new();

            let (val_chunks, _remainder) = val.as_chunks::<16>();

            // TODO This must be slow af, pls fix
            // TODO iterator magic? pls be faster? maybe separate panel would be better
            if blob_with_hex {
                for chunk in val_chunks {
                    let mut ascii_part = String::new();
                    let mut hex_part = String::new();
                    for c in chunk {
                        let hex_str = format!("{:02X}", *c);
                        hex_part.push_str(format!(" {}", hex_str).as_str());

                        if !c.is_ascii_graphic() {
                            ascii_part.push('.');
                        } else {
                            ascii_part.push(char::from_u32((*c).into()).unwrap());
                        }

                    }
                    result.push_str(format!("{} |{}\n", hex_part, ascii_part).as_str());
                }
            } else {
                for chunk in val_chunks {
                    let mut ascii_part = String::new();
                    for c in chunk {
                        if !c.is_ascii_graphic() {
                            ascii_part.push('.');
                        } else {
                            ascii_part.push(char::from_u32((*c).into()).unwrap());
                        }

                    }
                    result.push_str(ascii_part.as_str());
                }
            }

            result
        },
    };

    if was_cut {
        parsed.push('…');
    }

    parsed
}

impl RdbData {
    pub fn new() -> Result<Self, rocksdb::Error> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Duration: {:?}", duration);
        }
        let opts = rocksdb::Options::default();
        const PATH: &str = "temp_base";

        let cf_names = rocksdb::DB::list_cf(&opts, PATH)?;

        let error_if_log_file_exists = false; // should be true, but fucking rocks does not clean up itself properly
        let db = rocksdb::DB::open_cf_for_read_only(&opts, PATH, &cf_names, error_if_log_file_exists)?;

        Ok(Self {
            cf_names,
            db,
        })
    }

    pub fn get_val(&self, cf_name: &str, key: &str) -> Result<String, rocksdb::Error> {
        let start = Instant::now();
        defer!{
            let duration = start.elapsed();
            println!("Value query time: {:?}", duration);
        }

        let cf_handle = self.db.cf_handle(cf_name).unwrap();
        let v = self.db.get_pinned_cf(cf_handle, key)?.unwrap();
        Ok(parse_val(&v, 2048, true))
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

            let val_str = parse_val(&val, 64, false);

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

    let rdb_data_src = Rc::new(RdbData::new()?);

    ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(NullData{}.get_kv("")).into());
    ui.global::<ListViewAdapter>().set_list_items(Rc::new(rdb_data_src.get_cfs()).into());

    let ui_handle = ui.as_weak();
    let rdb_data_src_clone = rdb_data_src.clone(); // huh, pls help
    ui.on_change_db_value_preview(move |cf, key| {
        let ui = ui_handle.unwrap();
        let val = rdb_data_src_clone.get_val(cf.as_str(), key.as_str()).unwrap();
        ui.set_db_value_preview(val.into());
    });

    let ui_handle = ui.as_weak();
    let rdb_data_src_clone = rdb_data_src.clone(); // huh, pls help
    ui.on_change_column_family(move |new_cf|{
        let ui = ui_handle.unwrap();
        ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(rdb_data_src_clone.get_kv(new_cf.as_str())).into());
    });

    ui.run()?;

    Ok(())
}