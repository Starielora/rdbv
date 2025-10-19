use std::time::Instant;

use scopeguard::defer;

fn main()  -> Result<(), rocksdb::Error> {
    let start = Instant::now();
    defer!{
        let duration = start.elapsed();
        println!("Duration: {:?}", duration);
    }
    let opts = rocksdb::Options::default();
    const PATH: &str = "temp_base";

    let cf_names = rocksdb::DB::list_cf(&opts, PATH)?;

    let db = rocksdb::DB::open_cf_for_read_only(&opts, PATH, &cf_names, true)?;

    let cf_handles: Vec<&rocksdb::ColumnFamily> = cf_names.iter().map(|name|{ db.cf_handle(name).unwrap() }).collect();

    for cf_handle in cf_handles {
        println!("{:?}", db.get_column_family_metadata_cf(cf_handle).name);

        let mut it = db.raw_iterator_cf(cf_handle);
        it.seek_to_first();

        while it.valid() {
            println!("\t{:?}: {:?}", std::str::from_utf8(it.key().unwrap()).unwrap(), std::str::from_utf8(it.value().unwrap()).unwrap());
            it.next();
        }
    }

    Ok(())
}