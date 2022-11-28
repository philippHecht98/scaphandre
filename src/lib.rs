//! Scaphandre is an extensible monitoring agent for energy consumption metrics.
//!
//! It gathers energy consumption data from the system or other data sources thanks to components called *sensors*.
//!
//! Final monitoring data is sent to or exposed for monitoring tools thanks to *exporters*.
#[macro_use]
extern crate log;
pub mod exporters;
pub mod sensors;
use clap::ArgMatches;
use colored::*;
use exporters::VMconfiguration;
use exporters::{
    qemu::QemuExporter, Exporter
};
use sensors::{powercap_rapl::PowercapRAPLSensor, Sensor};
use std::collections::HashMap;
use std::io::{prelude::*, BufReader};
use std::net::{TcpListener};
use std::time::{Duration, SystemTime};

/// Helper function to get an argument from ArgMatches
fn get_argument(matches: &ArgMatches, arg: &'static str) -> String {
    if let Some(value) = matches.value_of(arg) {
        return String::from(value);
    }
    panic!("Couldn't get argument {}", arg);
}

/// Helper function to get a Sensor instance from ArgMatches
fn get_sensor(matches: &ArgMatches) -> Box<dyn Sensor> {
    let sensor = match &get_argument(matches, "sensor")[..] {
        "powercap_rapl" => PowercapRAPLSensor::new(
            get_argument(matches, "sensor-buffer-per-socket-max-kB")
                .parse()
                .unwrap(),
            get_argument(matches, "sensor-buffer-per-domain-max-kB")
                .parse()
                .unwrap(),
            matches.is_present("vm"),
        ),
        _ => PowercapRAPLSensor::new(
            get_argument(matches, "sensor-buffer-per-socket-max-kB")
                .parse()
                .unwrap(),
            get_argument(matches, "sensor-buffer-per-domain-max-kB")
                .parse()
                .unwrap(),
            matches.is_present("vm"),
        ),
    };
    Box::new(sensor)
}

/// Matches the sensor and exporter name and options requested from the command line and
/// creates the appropriate instances. Launchs the standardized entrypoint of
/// the choosen exporter: run()
/// This function should be updated to take new exporters into account.
pub fn run(matches: ArgMatches) {
//    loggerv::init_with_verbosity(matches.occurrences_of("v")).unwrap();

    let mut header = true;
    if matches.is_present("no-header") {
        header = false;
    }

    let sensor_boxed = get_sensor(&matches);

    if header {
        scaphandre_header("qemu");
    }

    let configurations = [
        VMconfiguration{host_name: String::from("small"), vcpu: 4, ram: 2048}
        ];

    let exporter_parameters;
    if let Some(qemu_exporter_parameters) = matches.subcommand_matches("qemu") {
        exporter_parameters = qemu_exporter_parameters.clone();
    } else {
        exporter_parameters = ArgMatches::default();
    }

    let exporter = &mut QemuExporter::new(sensor_boxed);

    let listener = TcpListener::bind("0.0.0.0:4444").unwrap();

    

    for configuration in configurations {

        let mut stream = listener.accept().unwrap().0;

        info!("Connection established\n");
        loop {
            let mut buf_reader = BufReader::new(&mut stream);
            let mut read_line = String::new();
            buf_reader.read_line(&mut read_line).unwrap();
            debug!("received: {}\n", read_line);
        
            if read_line.eq("finished recording\n") {
                print!("finished testing");
                break;
            } else if read_line.eq("startTestReq\n") {
                info!("start recording\n");

                stream.write(b"ack\n").unwrap();
                stream.flush().unwrap();

                exporter.run(&exporter_parameters, &configuration);
                //record_vm(exporter, &configuration, exporter_parameters.clone());

                stream.write(b"fin\n").unwrap();
                stream.flush().unwrap();
            } else {
                panic!("recieved wrong package");
            }
        }
    }
}



/// Returns options needed for each exporter as a HashMap.
/// This function has to be updated to enable a new exporter.
pub fn get_exporters_options() -> HashMap<String, Vec<clap::Arg<'static, 'static>>> {
    let mut options = HashMap::new();
    options.insert(
        String::from("qemu"),
        exporters::qemu::QemuExporter::get_options(),
    );
    options
}

fn current_system_time_since_epoch() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
}

pub fn scaphandre_header(exporter_name: &str) {
    let title = format!("Scaphandre {} exporter", exporter_name);
    println!("{}", title.red().bold());
    println!("Sending ⚡ metrics");
}

//  Copyright 2020 The scaphandre authors.
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
