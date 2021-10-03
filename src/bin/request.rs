//! Module request
//! try to match fasta sequence with repsect to database
//! 
//! request --database [-d] dirname --query [-q]  requestdir --nbsearch [-n] nbanswers
//! 
//! - database is the name of directory containing hnsw dump files and seqdict dump
//! - requestdir is a file containing list of fasta file containing sequence to search for
//! 

// We can use the same structure as the tohnsw module
// We parse a directory and send to a thread that do sketching and query
// we must enforce that the sketching size is the same as used in the database, so SketcherParams
// must have been dumped also in database directory.
// TODO for now we pass it
// 



use clap::{App, Arg};

// for logging (debug mostly, switched at compile time in cargo.toml)
use env_logger::{Builder};

//
use std::time::{SystemTime};
use cpu_time::ProcessTime;
use std::path::Path;
use std::fs::OpenOptions;

use hnsw_rs::prelude::*;
use kmerutils::base::{sequence::*, Kmer32bit};
use kmerutils::sketching::*;

use archaea::utils::idsketch::{SeqDict, Id, IdSeq, SketcherParams};
//mod files;
use archaea::utils::files::{process_dir,process_file};


// install a logger facility
pub fn init_log() -> u64 {
    Builder::from_default_env().init();
    println!("\n ************** initializing logger *****************\n");    
    return 1;
}


/// this function does the sketching and hnsw store of a whole directory
/// dumpdir_path is the path to directory containing dump of Hnsw, database sequence dictionary and sketchparams.
/// request_dirpath is the directory containing the fasta files which are the requests.
fn sketch_and_request_dir(request_dirpath : &Path, hnsw : &Hnsw<u32,DistHamming>, sketcher_params : &SketcherParams, knbn : usize, ef_search : usize) {
    //
    log::trace!("sketch_and_request_dir processing dir {}", request_dirpath.to_str().unwrap());
    log::info!("sketch_and_request_dir {}", request_dirpath.to_str().unwrap());
    let start_t = SystemTime::now();
    let cpu_start = ProcessTime::now();
    // a queue of signature request , size must be sufficient to benefit from threaded probminhash and search
    let request_block_size = 500;
    let mut request_queue : Vec<IdSeq>= Vec::with_capacity(request_block_size);
    //
    // Sketcher allocation, we do not need reverse complement hashing as we sketch assembled genomes. (Jianshu Zhao)
    //
    let kmer_hash_fn = | kmer : &Kmer32bit | -> u32 {
        let hashval = probminhash::invhash::int32_hash(kmer.0);
        hashval
    };
    let sketcher = seqsketchjaccard::SeqSketcher::new(sketcher_params.get_kmer_size(), sketcher_params.get_sketch_size());
    // to send IdSeq to sketch from reading thread to sketcher thread
    let (send, receive) = crossbeam_channel::bounded::<Vec<IdSeq>>(10_000);
    // launch process_dir in a thread or async
    crossbeam_utils::thread::scope(|scope| {
        // sequence sending, productor thread
        let mut nb_sent = 0;
        let sender_handle = scope.spawn(move |_|   {
            let res_nb_sent = process_dir(request_dirpath, &process_file, &send);
            match res_nb_sent {
                Ok(nb_really_sent) => {
                    nb_sent = nb_really_sent;
                    println!("process_dir processed nb sequences : {}", nb_sent);
                }
                Err(_) => {
                    println!("some error occured in process_dir");
                }
            };
            drop(send);
            Box::new(nb_sent)
        });
        // sequence reception, consumer thread
        let receptor_handle = scope.spawn(move |_| {
            let mut seqdict = SeqDict::new(100000);
            // we must read messages, sketch and insert into hnsw
            let mut read_more = true;
            while read_more {
                // try read, if error is Disconnected we stop read and both threads are finished.
                let res_receive = receive.recv();
                match res_receive {
                    Err(_) => { read_more = false;
                        // sketch the content of  insertion_queue if not empty
                        if request_queue.len() > 0 {
                            let sequencegroup_ref : Vec<&Sequence> = request_queue.iter().map(|s| s.get_sequence()).collect();
                            // collect rank
                            let seq_rank :  Vec<usize> = request_queue.iter().map(|s| s.get_rank()).collect();
                            // collect Id
                            let mut seq_id :  Vec<Id> = request_queue.iter().map(|s| Id::new(s.get_path(), s.get_fasta_id())).collect();
                            seqdict.0.append(&mut seq_id);
                            let signatures = sketcher.sketch_probminhash3a_kmer32bit(&sequencegroup_ref, kmer_hash_fn);                            
                            // we have Vec<u32> signatures we must go back to a vector of IdSketch for hnsw insertion
                            let mut data_for_hnsw = Vec::<(&Vec<u32>, usize)>::with_capacity(signatures.len());
                            for i in 0..signatures.len() {
                                data_for_hnsw.push((&signatures[i], seq_rank[i]));
                            }
                            // TODO parallel search
//                            hnsw.parallel_insert(&data_for_hnsw);
                        }
                    }
                    Ok(mut idsequences) => {
                        // concat the new idsketch in insertion queue.
                        request_queue.append(&mut idsequences);
                        // if request_queue is beyond threshold size we can go to threaded sketching and threading insertion
                        if request_queue.len() > request_block_size {
                            let sequencegroup_ref : Vec<&Sequence> = request_queue.iter().map(|s| s.get_sequence()).collect();
                            let seq_rank :  Vec<usize> = request_queue.iter().map(|s| s.get_rank()).collect();
                            // collect Id
                            let mut seq_id :  Vec<Id> = request_queue.iter().map(|s| Id::new(s.get_path(), s.get_fasta_id())).collect();
                            seqdict.0.append(&mut seq_id);
                            // computes hash signature
                            let signatures = sketcher.sketch_probminhash3a_kmer32bit(&sequencegroup_ref, kmer_hash_fn);
                            // we have Vec<u32> signatures we must go back to a vector of IdSketch, inserting unique id, for hnsw insertion
                            let mut data_for_hnsw = Vec::<(&Vec<u32>, usize)>::with_capacity(signatures.len());
                            for i in 0..signatures.len() {
                                data_for_hnsw.push((&signatures[i], seq_rank[i]));
                            }
                            // TODO parallel search
                            let knn_neighbours  = hnsw.parallel_search(&anndata.test_data, knbn, ef_search);

//                            hnsw.parallel_insert(&data_for_hnsw);
                            // we reset insertion_queue
                            request_queue.clear();
                        }
                    }
                }
            }
            //

            //
            Box::new(seqdict.0.len())
        }); // end of receptor thread
        // now we must join handles
        let nb_sent = sender_handle.join().unwrap();
        let nb_received = receptor_handle.join().unwrap();
        log::debug!("sketch_and_request_dir, nb_sent = {}, nb_received = {}", nb_sent, nb_received);
        if nb_sent != nb_received {
            log::error!("an error occurred  nb msg sent : {}, nb msg received : {}", nb_sent, nb_received);
        }
    }).unwrap();  // end of scope
    //
    let cpu_time = cpu_start.elapsed().as_secs();
    log::info!("process_dir : cpu time(s) {}", cpu_time);
    let elapsed_t = start_t.elapsed().unwrap().as_secs() as f32;
    log::info!("process_dir : elapsed time(s) {}", elapsed_t);
} // end of sketch_and_request_dir 



/// reload hnsw from dump directory
/// We know filename : hnswdump.hnsw.data and hnswdump.hnsw.graph
fn reload_hnsw(dump_dirpath : &Path) -> Option<Hnsw<u32, DistHamming>> {
    // just concat dirpath to filenames and get pathbuf
    let graph_path = dump_dirpath.join("/hnswdump.hnsw.graph");
    let graphfile = OpenOptions::new().read(true).open(&graph_path);
    if graphfile.is_err() {
        println!("test_dump_reload: could not open file {:?}", graph_path.as_os_str());
        std::panic::panic_any("test_dump_reload: could not open file".to_string());            
    }
    let graphfile = graphfile.unwrap();
    //
    let data_path = dump_dirpath.join("/hnswdump.hnsw.data");
    //  
    None
} // end of reload_hnsw



fn main() {
    let _ = init_log();
    //
    let matches = App::new("request")
        .arg(Arg::with_name("request_dir")
            .long("reqdir")
            .short("r")
            .takes_value(true)
            .help("name of directory containing request genomes to index"))
        .arg(Arg::with_name("database_dir")
            .long("datadir")
            .short("b")
            .takes_value(true)
            .help("name of directory containing database reference genomes"))            
        .arg(Arg::with_name("kmer_size")
            .long("kmer")
            .short("k")
            .takes_value(true)
            .help("expecting a kmer size"))
        .arg(Arg::with_name("sketch size")
            .long("sketch")
            .short("s")
            .default_value("8")
            .help("size of probinhash sketch, default to 8"))
        .arg(Arg::with_name("neighbours")
            .long("nbng")
            .short("n")
            .takes_value(true)
            .help("must specify number of neighbours in hnsw"))
        .get_matches();

        // decode matches, check for request_dir
        let request_dir;
        if matches.is_present("request_dir") {
            println!("decoding argument dir");
            request_dir = matches.value_of("reqdir").ok_or("").unwrap().parse::<String>().unwrap();
            if request_dir == "" {
                println!("parsing of request_dir failed");
                std::process::exit(1);
            }
        }
        else {
            println!("-r request_dir is mandatory");
            std::process::exit(1);
        }
        let request_dirpath = Path::new(&request_dir);
        if !request_dirpath.is_dir() {
            println!("error not a directory : {:?}", request_dirpath);
            std::process::exit(1);
        }

        // parse database dir
        let database_dir;
        if matches.is_present("database_dir") {
            println!("decoding argument dir");
            database_dir = matches.value_of("datadir").ok_or("").unwrap().parse::<String>().unwrap();
            if database_dir == "" {
                println!("parsing of database_dir failed");
                std::process::exit(1);
            }
        }
        else {
            println!("-r database_dir is mandatory");
            std::process::exit(1);
        }
        let database_dirpath = Path::new(&database_dir);
        if !database_dirpath.is_dir() {
            println!("error not a directory : {:?}", database_dirpath);
            std::process::exit(1);
        }



        // get sketching params
        let mut sketch_size = 96;
        if matches.is_present("size") {
            sketch_size = matches.value_of("size").ok_or("").unwrap().parse::<u16>().unwrap();
            println!("do you know what you are doing, sketching size {}", sketch_size);
        }
        else {
            println!("will use dumped sketch size");
        }
        //
        let mut kmer_size = 8;
        if matches.is_present("kmer_size") {
            kmer_size = matches.value_of("size").ok_or("").unwrap().parse::<u16>().unwrap();
            println!("kmer size {}", kmer_size);
        }
        else {
            println!("will use dumped kmer size");
        }

        // in fact sketch_params must be initialized from the dump directory
        let sketch_params =  SketcherParams::new(kmer_size as usize, sketch_size as usize);  
        //
        let nbng;
        if matches.is_present("neighbours") {
            nbng = matches.value_of("neighbours").ok_or("").unwrap().parse::<u16>().unwrap();
            println!("nb neighbours you will need in hnsw requests {}", nbng);
        }        
        else {
            println!("-n nbng is mandatory");
            std::process::exit(1);
        }
        //
        // Do all dump reload, first Hns
        //
        let hnsw = reload_hnsw(database_dirpath);
        let hnsw = match hnsw {
            Some(hnsw) => hnsw,
            _ => {
                panic!("hnsw reload from dump dir {} failed", database_dirpath.to_str().unwrap());
            }
        };
        // reload Sketchparams
        // reloat SeqDict
        // 
        let ef_search = 200;
        sketch_and_request_dir(&request_dirpath, &hnsw, &sketch_params, nbng as usize, ef_search);

}  // end of main