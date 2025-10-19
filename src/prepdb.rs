use std::time::Instant;

use scopeguard::defer;

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
