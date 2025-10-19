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
    fn get_kv(&self) -> VecModel<slint::ModelRc<StandardListViewItem>>;
    fn get_cfs(&self) -> VecModel<StandardListViewItem>;
}

struct DummyData { }

impl SlintDataSrc for DummyData {
    fn get_kv(&self) -> VecModel<ModelRc<StandardListViewItem>> {
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
}

impl SlintDataSrc for RdbData {
    fn get_kv(&self) -> VecModel<slint::ModelRc<StandardListViewItem>> {
        let cf_names = &self.cf_names;
        let db = &self.db;

        let cf_handles: Vec<&rocksdb::ColumnFamily> = cf_names.iter().map(|name|{ db.cf_handle(name).unwrap() }).collect();

        let cf_handle = cf_handles.get(1).unwrap();

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

    // let data_src = DummyData{};
    let data_src = RdbData::new()?;

    ui.global::<TableViewPageAdapter>().set_row_data(Rc::new(data_src.get_kv()).into());
    ui.global::<ListViewAdapter>().set_list_items(Rc::new(data_src.get_cfs()).into());

    let ui_weak = ui.as_weak();
    ui.on_change_db_value_preview(move |current_row| {
        println!("I'm in rust now: {}", current_row);

        let ui = ui_weak.unwrap();
        let item = ui.global::<TableViewPageAdapter>().get_row_data().row_data(current_row as usize).unwrap();

        // magic values, will break easily
        let key = item.row_data(0).unwrap().text;
        let ui_value = item.row_data(1).unwrap().text;

        ui.set_db_value_preview(slint::format!("Value for key: \"{}\" is \"{}\"", key, ui_value));
    });

    ui.run()?;

    Ok(())
}