use crate::exporters::Exporter;
use crate::sensors::utils::current_system_time_since_epoch;
use crate::sensors::{utils::ProcessRecord, Sensor, Topology};
use std::collections::HashMap;
use std::fmt::{format, self};
use std::time::Duration;
use std::{fs, io, thread, time, vec};

/// An Exporter that extracts power consumption data of running
/// Qemu/KVM virtual machines on the host and store those data
/// as folders and files that are supposed to be mounted on the
/// guest/virtual machines. This allow users of the virtual machines
/// to collect and deal with their power consumption metrics, the same way
/// they would do it if they managed bare metal machines.

pub struct TestCase {
    test_name: String,
    vms: HashMap<String, Vec<f64>>, 
    temps: Vec<f64>,
    start_recording: Duration, 
    end_recording: Duration, 
    store_base_path: String,
}

impl TestCase {
    pub fn new(
        test_case_name: String, 
        store_base_path: String, 
    ) -> TestCase {
        TestCase { 
            test_name: test_case_name, 
            vms: HashMap::new(), 
            temps: Vec::new(), 
            start_recording: Duration::new(0, 0), 
            end_recording: Duration::new(0, 0),
            store_base_path: store_base_path,
        }
    }

    pub fn start_recording(&mut self) {
        self.start_recording = current_system_time_since_epoch();
    }

    pub fn stop_recording(&mut self) {
        self.end_recording = current_system_time_since_epoch();
    }

    pub fn add_temp_measurement(&mut self, temperature: f64) {
        self.temps.push(temperature);
    }

    pub fn add_energy(&mut self, vm_name: String, uj: f64) {
        if let Some(measurements) = self.vms.get_mut(&vm_name) {
            measurements.push(uj);
        } else {
            // vm is not in the map
            let mut vector: Vec<f64> = Vec::new();
            vector.push(uj);
            self.vms.insert(vm_name, vector);
        }
    }

    pub fn get_avg_temp(&mut self) -> f64 {
        let mut sum: f64 = 0.0;
        for entry in self.temps.clone() {
            sum += entry;
        }
        return sum / self.temps.len() as f64;
    }

    pub fn get_test_duration(&mut self) -> Duration {
        debug!("test duration: {:?}", self.end_recording - self.start_recording);
        self.end_recording - self.start_recording
    }

    pub fn get_watt_consumed_by_vm(&mut self, vm_name: String) -> f64 {
        let entries = self.vms.get(&vm_name).unwrap();
        let mut sum: f64 = 0.0;
        for entry in entries {
            sum += *entry
        }

        return sum / self.get_test_duration().as_secs_f64();
    }

    pub fn store_test_data(&mut self) {
        for vm in self.vms.clone().iter() {
            // write energy consumption
            add_or_create_file_with_value(
                format!("{}/{}", self.store_base_path, *vm.0), 
                String::from("consumed_watt"), 
                self.get_watt_consumed_by_vm(vm.0.clone()));

            // write avg temp
            add_or_create_file_with_value(
                format!("{}/{}", self.store_base_path, *vm.0), 
                String::from("avg_temp"), 
                self.get_avg_temp());
        }

    }
}


impl fmt::Display for TestCase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Testcase: test_name: {},\n\tvms: {:?}, \n\ttemps: {:?}, \n\tstart_recording: {:?}, \n\tend_recording: {:?}",
            self.test_name,
            self.vms,
            self.temps, 
            self.start_recording, 
            self.end_recording, 
        )
    }
}


pub struct QemuExporter {
    topology: Topology,
}


impl Exporter for QemuExporter {
    /// Runs iteration() in a loop.
    fn run(&mut self, _parameters: &clap::ArgMatches, test_case_name: &String) {
        info!("Starting qemu exporter");
        let cleaner_step = 10;
        let path = format!("{}/{}", "/var/lib/libvirt/mount/scaphandre", test_case_name);
        info!("directory for storing {}", path);

        let mut test_case = TestCase::new(String::clone(test_case_name), path);
        let sleep_time = time::Duration::from_secs(1);

        // warm up machine
        //thread::sleep(time::Duration::from_secs(10));

        test_case.start_recording();
        for _ in 0..cleaner_step+1 {
            self.iteration(&mut test_case);
            thread::sleep(sleep_time);
        }
        self.iteration(&mut test_case);
        test_case.stop_recording();
        
        debug!("store data");
        debug!(test_case);
        test_case.store_test_data();

        self.topology
            .proc_tracker
            .clean_terminated_process_records_vectors();
    }

    fn get_options() -> Vec<clap::Arg<'static, 'static>> {
        Vec::new()
    }
}

impl QemuExporter {
    /// Instantiates and returns a new QemuExporter
    pub fn new(mut sensor: Box<dyn Sensor>) -> QemuExporter {
        let some_topology = *sensor.get_topology();

        QemuExporter {
            topology: some_topology.unwrap()
        }
    }

    /// Performs processing of metrics, using self.topology
    pub fn iteration(&mut self, test_case: &mut TestCase){
        let path = String::from("/var/lib/libvirt/mount/scaphandre/");
        trace!("path: {}", path);
        self.topology.refresh();
        let topo_uj_diff = self.topology.get_records_diff();
        let topo_stat_diff = self.topology.get_stats_diff();
        if let Some(topo_rec_uj) = topo_uj_diff {
            debug!("Got topo uj diff: {:?}", topo_rec_uj);
            debug!("Got Joule of hole system: {:?}", topo_rec_uj.value.parse::<f64>().unwrap() / (1000 as f64 * 1000 as f64));
            let proc_tracker = self.topology.get_proc_tracker();
            let processes = proc_tracker.get_alive_processes();
            let qemu_processes = QemuExporter::filter_qemu_vm_processes(&processes);
            debug!(
                "Number of filtered qemu processes: {}",
                qemu_processes.len()
            );
            for qp in qemu_processes {
                if qp.len() > 2 {
                    let last = qp.first().unwrap();
                    let previous = qp.get(1).unwrap();
                    let vm_name =
                        QemuExporter::get_vm_name_from_cmdline(&last.process.cmdline().unwrap());
                    let time_pdiff = last.total_time_jiffies() - previous.total_time_jiffies();

                    if let Some(time_tdiff) = &topo_stat_diff {
                        /*
                        let first_domain_path = format!("{}/{}/intel-rapl:0:0", path, vm_name);
                        if fs::read_dir(&first_domain_path).is_err() {
                            match fs::create_dir_all(&first_domain_path) {
                                Ok(_) => debug!("Created {} folder.", &path),
                                Err(error) => panic!("Couldn't create {}. Got: {}", &path, error),
                            }
                        }
                        */
                        
                        let tdiff = time_tdiff.total_time_jiffies();
                        trace!("Time_pdiff={} time_tdiff={}", time_pdiff.to_string(), tdiff);
                        let ratio = (time_pdiff as f64) / (tdiff as f64);
                        debug!("messed {} uJ difference to last timestamp", topo_rec_uj.value.parse::<f64>().unwrap());
                        debug!("Ratio is {}", ratio.to_string());
                        let uj_to_add = ratio * topo_rec_uj.value.parse::<f64>().unwrap();
                        
                        debug!("adding {} uJ", uj_to_add); 
                        test_case.add_energy(vm_name, uj_to_add);

                    } 
                }
            }
            test_case.add_temp_measurement(self.read_temp());
        }
    }

    fn read_temp(&mut self) -> f64 {
        let base_path = String::from("/sys/class/thermal");

        let mut temp: f64 = 0.0;

        if let Some(thermal_sensors) = fs::read_dir(&base_path).ok() {
            let mut count = 0;
            for mut sensor in thermal_sensors {
                
                if sensor.as_ref().unwrap().file_name().into_string().unwrap().contains("cooling") {
                    continue;
                }
                if let Ok(temperature) = fs::read_to_string(
                    format!("{}/temp", sensor.as_mut().unwrap().path().display())) {
                        debug!("messed temperature for device {}: {}", sensor.as_mut().unwrap().path().display(), temperature);
                        temp += temperature.strip_suffix('\n').unwrap().parse::<f64>().unwrap();
                        count += 1;
                }
            }
            return temp / count as f64
        } else {
            error!("couln't read in temperature values")
        }
        return 0.0
        /*
        for socket in self.topology.get_sockets_passive() {
            let temp_sensor_path = format!("{}/thermal_zone{}/temp", base_path, socket.id + 1);
            if let Ok(temperature) = fs::read_to_string(&temp_sensor_path) {
                temp += temperature.parse::<f64>().unwrap();
            }
        }
        let num_sockets = self.topology.get_sockets_passive().len() as u16;
                
        return temp / num_sockets as f64
        */

    }

    /// Parses a cmdline String (as contained in procs::Process instances) and returns
    /// the name of the qemu virtual machine if this process is a qemu/kvm guest process
    fn get_vm_name_from_cmdline(cmdline: &[String]) -> String {
        for elmt in cmdline {
            if elmt.starts_with("guest=") {
                let mut splitted = elmt.split('=');
                splitted.next();
                return String::from(splitted.next().unwrap().split(',').next().unwrap());
            }
        }
        String::from("")
    }

    /// Either creates an energy_uj file (as the ones managed by powercap kernel module)
    /// in 'path' and adds 'uj_value' to its numerical content, or simply performs the
    /// addition if the file exists.
    fn add_or_create_energy_file(path: &str, uj_value: f64) -> io::Result<()> {
        let mut content = 0.0;
        if fs::read_dir(path).is_err() {
            match fs::create_dir_all(path) {
                Ok(_) => debug!("Created {} folder.", path),
                Err(error) => panic!("Couldn't create {}. Got: {}", path, error),
            }
        }
        let file_path = format!("{}/{}", path, "energy_uj");
        if let Ok(file) = fs::read_to_string(&file_path) {
            content = file.parse::<f64>().unwrap();
            content += uj_value;
        }
        fs::write(file_path, content.to_string())
    }

    /// Filters 'processes' to match processes that look like qemu/kvm guest processes.
    /// Returns what was found.
    fn filter_qemu_vm_processes(processes: &[&Vec<ProcessRecord>]) -> Vec<Vec<ProcessRecord>> {
        let mut qemu_processes: Vec<Vec<ProcessRecord>> = vec![];
        trace!("Got {} processes to filter.", processes.len());
        for vecp in processes.iter() {
            if !vecp.is_empty() {
                if let Some(pr) = vecp.get(0) {
                    if let Ok(cmdline) = pr.process.cmdline() {
                        if let Some(res) = cmdline.iter().find(|x| x.contains("qemu-system")) {
                            debug!("Found a process with {}", res);
                            let mut tmp: Vec<ProcessRecord> = vec![];
                            for p in vecp.iter() {
                                tmp.push(p.clone());
                            }
                            qemu_processes.push(tmp);
                        }
                    }
                }
            }
        }
        qemu_processes
    }
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

fn add_or_create_file_with_value(path: String, file_name: String, value: f64) {
    if fs::read_dir(&path).is_err() {
        match fs::create_dir_all(&path) {
            Ok(_) => debug!("Created {} folder.", path.clone()),
            Err(error) => panic!("Couldn't create {}. Got: {}", path, error),
        }
    }
    fs::write(format!("{}/{}", path, file_name), value.to_string()).unwrap();
}
