use std::{ time::Duration};

use saopaulio::{m0_task::{ Runtime}, m1_time::sleep::Sleep, m3_timeout::Timeout};

 
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

            let _timeout = Timeout::new(
                async{
                    println!("\nmain()block_on()future block: timeout test started");

                    Sleep::sleep(Duration::from_secs(10)).await;

                    println!("\nmain()block_on()future block: timeout test Future won against sleep");
                },
                Sleep::sleep(Duration::from_secs(5)),
            ).await;

            println!("\nmain()block_on()future block: inner runtime context test end");

        },
    );

    Runtime::run_until_idle(&executor);
}