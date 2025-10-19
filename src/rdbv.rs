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

        let db = rocksdb::DB::open_cf_for_read_only(&opts, PATH, &cf_names, true)?;

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
        let v = self.db.get_cf(cf_handle, key)?.unwrap();
        Ok(String::from_utf8(v).unwrap())
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
            let val = std::str::from_utf8(it.value().unwrap()).unwrap();

            let items = Rc::new(VecModel::default());
            items.push(key.into());
            items.push(val.into());

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