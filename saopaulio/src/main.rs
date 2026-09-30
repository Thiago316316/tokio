use std::time::Duration;

use saopaulio::{m0_task::{ Runtime}, m1_time::sleep::Sleep};

 
fn main (){
    //m0_task::main();

     let executor = Runtime::new();
    executor.block_on(
        async {
            println!("\nmain()block_on()future block: main future started");

            Runtime::spawn(async {
                println!("task A");
                Runtime::spawn(async {
                    println!("task B");
                });
            });

            Sleep::sleep(Duration::from_secs(1)).await;

            println!("\nmain()block_on()future block: inner runtime context test end");

        },
    );
    
    Runtime::run_until_idle(&executor);
}