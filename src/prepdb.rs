use std::time::Instant;

use scopeguard::defer;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "assets/"]
struct Assets;


const COLUMN_FAMILIES: [&'static str; 3] = ["Job", "Setup", "Scan"];

fn setup_column_families(db: &mut rocksdb::DB) -> Result<(), rocksdb::Error> {
    let opts = rocksdb::Options::default();

    for name in COLUMN_FAMILIES {
        db.create_cf(name, &opts)?;
    }

    Ok(())
}

fn put_job_shit(db: &rocksdb::DB, cf: &rocksdb::ColumnFamily) -> Result<(), rocksdb::Error> {

    db.put_cf(cf, "Job_k1", "Job_v1")?;
    db.put_cf(cf, "Job_k2", "Job_v2")?;
    db.put_cf(cf, "Job_k3", "Job_v3")?;
    let text = r#"commonly used names of animals and plants, such as トカゲ (tokage, "lizard"), ネコ (neko, "cat") and バラ (bara, "rose"), and certain other technical and scientific terms, including chemical and mineral names such as カリウム (kariumu, "potassium"), ポリマー (porimā, "polymer") and ベリル (beriru, "beryl")"#;
    db.put_cf(cf, "Job_k3", text)?;

    let img = Assets::get("satisfactory-753268177.jpg").unwrap();
    db.put_cf(cf, "image", img.data.as_ref())?;

    let json = r#"{"name": "John Doe","age": 43,"phones": ["+44 1234567","+44 2345678"]}"#;
    let json2 = r#"
        {
            "name": "John Doe",
            "age": 43,
            "phones": [
                "+44 1234567",
                "+44 2345678"
            ]
        }"#;

    db.put_cf(cf, "json", json)?;
    db.put_cf(cf, "json formatted", json2)?;

    Ok(())
}

fn put_setup_shit(db: &rocksdb::DB, cf: &rocksdb::ColumnFamily) -> Result<(), rocksdb::Error> {

    db.put_cf(cf, "Setup_k1", "Setup_v1")?;
    db.put_cf(cf, "Setup_k2", "Setup_v2")?;
    db.put_cf(cf, "Setup_k3", "Setup_v3")?;

    Ok(())
}

fn put_scan_shit(db: &rocksdb::DB, cf: &rocksdb::ColumnFamily) -> Result<(), rocksdb::Error> {

    db.put_cf(cf, "Scan_k1", "Scan_v1")?;
    db.put_cf(cf, "Scan_k2", "Scan_v2")?;
    db.put_cf(cf, "Scan_k3", "Scan_v3")?;

    let img = Assets::get("satisfactory-753268177.jpg").unwrap();
    for i in 0..1024 {
        db.put_cf(cf, format!("img {}", i), img.data.as_ref())?;
    }

    Ok(())
}

fn put_shit(db: &rocksdb::DB) -> Result<(), rocksdb::Error> {

    let job_cf = db.cf_handle(COLUMN_FAMILIES[0]).unwrap();
    let setup_cf = db.cf_handle(COLUMN_FAMILIES[1]).unwrap();
    let scan_cf = db.cf_handle(COLUMN_FAMILIES[2]).unwrap();

    put_job_shit(db, job_cf)?;
    put_setup_shit(db, setup_cf)?;
    put_scan_shit(db, scan_cf)?;

    Ok(())
}

fn main() -> Result<(), rocksdb::Error> {
    let start = Instant::now();
    defer!{
        let duration = start.elapsed();
        println!("Duration: {:?}", duration);
    }

    let mut opts = rocksdb::Options::default();
    opts.create_if_missing(true);
    let mut db = rocksdb::DB::open(&opts, "temp_base")?;

    setup_column_families(&mut db)?;
    put_shit(&db)?;
    let mut compact_opts = rocksdb::WaitForCompactOptions::default();
    compact_opts.set_flush(true);
    db.wait_for_compact(&compact_opts)?;
    db.flush_wal(true)?;
    db.flush()?;

    Ok(())
}
