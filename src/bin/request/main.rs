//! Module request
//! try to match fasta sequence with repsect to database
//! 
//! request --database [-b] basedirname --query [-r]  requestdir --nbsearch [-n] nbanswers -s sketch_size [ann]
//! 
//! - database is the name of directory containing hnsw dump files and seqdict dump
//! - requestdir is a directory containing list of fasta file containing sequence to search for
//! 
//! [ann] is an optional subcommand asking for some statistics on distances between hnsw items
//! In fact as in basedirname there must be a file (processingparams.json) specifying sketch size and kmer size, these
//! 2 options are useless in standard mode.

// We can use the same structure as the tohnsw module
// We parse a directory and send to a thread that do sketching and query
// we must enforce that the sketching size is the same as used in the database, so SketcherParams
// must have been dumped also in database directory.
// 



use clap::{App, Arg, SubCommand};

// for logging (debug mostly, switched at compile time in cargo.toml)
use env_logger::{Builder};

//

use std::path::{Path};

use kmerutils::sketching::seqsketchjaccard::*;

//mod files;
use archaea::utils::*;
use archaea::dna::dnarequest::get_sequence_matcher;


// install a logger facility
pub fn init_log() -> u64 {
    Builder::from_default_env().init();
    println!("\n ************** initializing logger *****************\n");    
    return 1;
}






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
        .arg(Arg::with_name("seq")
            .long("seq")
            .takes_value(false)
            .help("--seq to get a processing by sequence"))
        .subcommand(SubCommand::with_name("ann")
            .about("annembed usage")
            .arg(Arg::with_name("stats")
                .takes_value(false)
                .long("stats")
                .short("s")
                .help("to get stats on nb neighbours"))
        )
        .get_matches();


        let mut ann_params = AnnParameters::new(false);
        match matches.subcommand() {
            ("ann", Some(ann_match)) => {
                log::info!("got ann subcommand");
                if ann_match.is_present("stats") {
                    println!(" got subcommand neighbour stats option");
                    ann_params = AnnParameters::new(true);
                }
            },
            ("", None)               => println!("no subcommand at all"),
            _                        => unreachable!(),
        }

        // by default we process files in one large sequence block
        // decode matches, check for request_dir
        let request_dir;
        if matches.is_present("request_dir") {
            println!("decoding argument dir");
            request_dir = matches.value_of("request_dir").ok_or("").unwrap().parse::<String>().unwrap();
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
            database_dir = matches.value_of("database_dir").ok_or("").unwrap().parse::<String>().unwrap();
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
        let mut kmer_size = 28;
        if matches.is_present("kmer_size") {
            kmer_size = matches.value_of("kmer_size").ok_or("").unwrap().parse::<u16>().unwrap();
            println!("kmer size {}", kmer_size);
        }
        else {
            println!("will use dumped kmer size");
        }
        // in fact sketch_params must be initialized from the dump directory
        let _sketch_params =  SeqSketcher::new(kmer_size as usize, sketch_size as usize);  
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
        // match subcommands

        //
        // matching args is finished
        //     
        let ef_search = 5000;
        log::info!("ef_search : {:?}", ef_search);
        let filter_params = FilterParams::new(0);
        //
        // Do all dump reload, first sketch params. We reload smaller files first 
        // so that path errors are found early 
        //
        let processing_params = ProcessingParams::reload_json(database_dirpath);
        let processing_params = match processing_params {
            Ok(processing_params) => processing_params,
            _ => {
                panic!("ProcessingParams reload from dump dir {} failed", database_dirpath.to_str().unwrap());
            }
        };
        let sk_params = processing_params.get_sketching_params();
        log::info!("sketch params reloaded kmer size : {}, sketch size {}", sk_params.get_kmer_size(), sk_params.get_sketch_size());
        //
        // reload processing state
        //
        let mut state_name = database_dir.clone();
        state_name.push_str("/processing_state.json");
        let proc_state_res = ProcessingState::reload_json(database_dirpath);
        let proc_state;
        if let Ok(_) = proc_state_res {
                proc_state = proc_state_res.unwrap();
                println!("reloaded processing state , nb sequences : {}", proc_state.nb_seq);
        }
        else {
                println!("could not reload processing state");
        }
        // reload SeqDict
        let mut seqname = database_dir.clone();
        seqname.push_str("/seqdict.json");
        log::info!("\n reloading sequence dictionary from {}", &seqname);
        let seqdict = SeqDict::reload(&seqname);
        let seqdict = match seqdict {
            Ok(seqdict ) => seqdict ,
            _ => {
                panic!("SeqDict reload from dump file  {} failed", seqname);
            }            
        };
        log::info!("reloading sequence dictionary from {} done", &seqname);
        // we have everything we want...
        if let Ok(mut seq_matcher) = get_sequence_matcher(request_dirpath, database_dirpath, &processing_params, 
                        &filter_params, &ann_params, &seqdict, nbng, ef_search) {
            if processing_params.get_block_flag() == false {
                log::info!("sequence mode, trying to analyze..");
                let _= seq_matcher.analyze();
            }
        }
        else {
            panic!("Error occurred in get_matcher");
        }
        // 
}  // end of main