use embassy_time::{Duration, Instant};

#[derive(Default)]
pub struct Table {
    addr_allocs: [Option<Element>; 7],
}

pub struct Element {
    serial: [u8; 8],
    last_msg: Instant,
}

pub enum PipeAlloc {
    New(u8),
    Existing(u8),
}

impl Table {
    pub const fn new() -> Self {
        Self {
            addr_allocs: [const { None }; 7],
        }
    }

    pub fn allocate_pipe(&mut self, serial: &[u8; 8]) -> Option<PipeAlloc> {
        let mut first_empty = None;
        for (i, s) in self.addr_allocs.iter_mut().enumerate() {
            if let Some(s) = s.as_ref() {
                if s.serial == *serial {
                    return Some(PipeAlloc::Existing((i as u8) + 1));
                }
            } else if first_empty.is_none() {
                first_empty = Some(i);
            }
        }
        if let Some(s) = first_empty {
            self.addr_allocs[s] = Some(Element {
                serial: *serial,
                last_msg: Instant::now(),
            });
            Some(PipeAlloc::New((s as u8) + 1))
        } else {
            None
        }
    }

    pub fn pipe_for_serial(&self, serial: &[u8; 8]) -> Option<u8> {
        for (i, s) in self.addr_allocs.iter().enumerate() {
            if let Some(s) = s.as_ref() {
                if s.serial == *serial {
                    return Some((i as u8) + 1);
                }
            }
        }
        None
    }

    pub fn pipe_valid(&self, pipe: u8) -> bool {
        if pipe == 0 {
            return false;
        }
        let pipe = pipe - 1;
        self.addr_allocs
            .get(pipe as usize)
            .is_some_and(|s| s.is_some())
    }

    pub fn serial_for_pipe(&self, pipe: u8) -> Option<[u8; 8]> {
        if pipe == 0 {
            return None;
        }
        let pipe = pipe - 1;
        let slot = self.addr_allocs.get(pipe as usize)?;
        let slot = slot.as_ref()?;
        Some(slot.serial)
    }

    pub fn update_time(&mut self, pipe: u8, serial: &[u8; 8]) -> bool {
        if pipe == 0 {
            return false;
        }
        let pipe = pipe - 1;
        let Some(slot) = self.addr_allocs.get_mut(pipe as usize) else {
            return false;
        };
        let Some(slot) = slot.as_mut() else {
            return false;
        };
        if slot.serial == *serial {
            slot.last_msg = Instant::now();
            true
        } else {
            false
        }
    }

    pub fn cull_older_than(&mut self, dur: Duration) {
        let now = Instant::now();
        for s in self.addr_allocs.iter_mut() {
            if let Some(sr) = s.as_ref() {
                let cds = now.checked_duration_since(sr.last_msg);
                if cds.is_some_and(|d| d >= dur) || cds.is_none() {
                    defmt::info!("Culling node");
                    *s = None;
                }
            }
        }
    }

    pub fn extract_table(&self, out: &mut heapless::Vec<[u8; 8], 7>) {
        out.clear();
        for s in self.addr_allocs.iter() {
            if let Some(sr) = s.as_ref() {
                let _ = out.push(sr.serial);
            }
        }
    }
}
