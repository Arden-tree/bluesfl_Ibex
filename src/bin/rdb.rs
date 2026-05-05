use regex::Regex;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

struct GDBController {
    gdb_output: std::process::ChildStdout,
    gdb_input: std::process::ChildStdin,
    program_output_regex: Regex,
}

impl GDBController {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut gdb_process = Command::new("gdb")
            .arg("--interpreter=mi2")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let gdb_output = gdb_process.stdout.take().unwrap();
        let gdb_input = gdb_process.stdin.take().unwrap();

        let program_output_regex = Regex::new(r#"~"(.*?)""#)?;

        let mut controller = Self {
            gdb_output,
            gdb_input,
            program_output_regex,
        };

        controller.execute_command("-gdb-set mi-async on")?;
        controller.execute_command(
            "-file-exec-and-symbols /home/lzz/hAPR/state_fuzz/cmake-build-debug/test_program",
        )?;

        Ok(controller)
    }

    fn execute_command(&mut self, command: &str) -> Result<(), Box<dyn std::error::Error>> {
        writeln!(self.gdb_input, "{}", command)?;

        thread::sleep(Duration::from_millis(100));

        let response = self.read_response()?;
        let output = self.extract_program_output(&response);

        if !output.is_empty() {
            println!("Target program output:\n{}", output);
        }

        Ok(())
    }

    fn read_response(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        let mut buffer = [0; 4096];
        let mut response = String::new();

        loop {
            match self.gdb_output.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let chunk = std::str::from_utf8(&buffer[..n])?;
                    response.push_str(chunk);

                    if chunk.contains("(gdb)") {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            }
        }

        Ok(response)
    }

    fn extract_program_output(&self, response: &str) -> String {
        let mut output = String::new();
        println!("response: {}", response);
        for cap in self.program_output_regex.captures_iter(response) {
            if let Some(_) = cap.get(1) {
                let cleaned = response
                    .replace(r#"\""#, r#"""#)
                    .replace(r#"\n"#, "\n")
                    .replace(r#"\\t"#, "\t");
                output.push_str(&cleaned);
            }
        }

        output
    }

    fn set_breakpoint(&mut self, location: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.execute_command(&format!("-break-insert {}", location))
    }

    fn run_program(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.execute_command("-exec-run")
    }

    fn set_variable(
        &mut self,
        var_name: &str,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.execute_command(&format!(
            "-data-evaluate-expression \"{} = {}\"",
            var_name, value
        ))
    }

    fn continue_execution(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.execute_command("-exec-continue")
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut gdb = GDBController::new()?;

    gdb.set_breakpoint("test_program.cpp:15")?;
    gdb.run_program()?;
    thread::sleep(Duration::from_millis(500));
    gdb.set_variable("x", "1")?;
    gdb.continue_execution()?;

    Ok(())
}
