use std::{path::Path, time::Duration};

use chrono::{Local, TimeDelta, Timelike};
use poststation_sdk::connect;
use template_icd::{DisplayCommand, DrawDisplay, SetDisplay};
use tokio::time::sleep;
use typst::{
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime},
    syntax::{FileId, Source},
    text::{Font, FontBook},
    utils::LazyHash,
    Library, World,
};
use typst_render::render;

pub struct FakeWorld {
    library: LazyHash<Library>,
    fontbook: LazyHash<FontBook>,
    text: Source,
    fonts: Vec<Font>,
}

impl FakeWorld {
    pub fn new(src: impl AsRef<str>) -> Self {
        let fonts = fonts(Path::new("."));
        Self {
            library: LazyHash::new(Library::default()),
            fontbook: LazyHash::new(FontBook::from_fonts(&fonts)),
            text: Source::detached(src.as_ref()),
            fonts,
        }
    }
}

impl World for FakeWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.fontbook
    }

    fn main(&self) -> FileId {
        self.text.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.text.id() {
            Ok(self.text.clone())
        } else {
            FileResult::Err(FileError::AccessDenied)
        }
    }

    fn file(&self, _id: FileId) -> FileResult<typst::foundations::Bytes> {
        todo!()
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        todo!()
    }
}

/// Helper function
fn fonts(root: &Path) -> Vec<Font> {
    std::fs::read_dir(root.join("fonts"))
        .expect("Could not read fonts from disk")
        .map(Result::unwrap)
        .flat_map(|entry| {
            let path = entry.path();
            let bytes = std::fs::read(&path).unwrap();
            let buffer = Bytes::from(bytes);
            let face_count = ttf_parser::fonts_in_collection(&buffer).unwrap_or(1);
            (0..face_count).map(move |face| {
                Font::new(buffer.clone(), face).unwrap_or_else(|| {
                    panic!("failed to load font from {path:?} (face index {face})")
                })
            })
        })
        .collect()
}

#[tokio::main]
async fn main() {
    let mut last = Local::now() - TimeDelta::minutes(1);
    let mut seq_ctr = 0u32;
    let seq_ctr = &mut seq_ctr;
    let mut ctr = || {
        let old = *seq_ctr;
        *seq_ctr = seq_ctr.wrapping_add(1);
        old
    };


    loop {
        loop {
            let now = Local::now();
            if now.hour() == last.hour() && now.minute() == last.minute() {
                sleep(Duration::from_secs(1)).await;
                continue;
            }
            last = now;
            break;
        }
        let h = last.hour();
        let m = last.minute();

        let src = format!(
            "
            #set page(
                width: 4.0in,  height: 3.0in,
                margin: 0.0in, fill: black,
            )
            #set text(font: \"Departure Mono\")
            #set align(center + horizon)
            #stack(
                dir: ttb,
                rect(width: 100%, height: 48%, fill: black)[
                    #set text(size: 1.0in, fill: white)
                    {h:02}:{m:02}
                ],
                rect(width: 90%, height: 4%, fill: white, radius: 0.1in),
                rect(width: 100%, height: 48%, fill: black, inset: 0.2in)[
                    #set text(size: 0.33in, fill: white)
                    #set align(left + horizon)
                    Temp: 4.2°C
                    #linebreak()
                    RH% : 51.0%
                ],
            )
        "
        );

        let world = FakeWorld::new(src);

        let document = typst::compile(&world).output.unwrap();
        println!("{:?}", document.info);
        println!("{:?}", document.pages.len());
        let page = &document.pages[0];
        println!("{:?}", page.frame.width());
        println!("{:?}", page.frame.height());
        let rend = render(page, 100.0 / 72.0);
        println!("{:?}", rend.width());
        println!("{:?}", rend.height());
        println!("{:?}", rend.data().len());
        println!("{:?}", rend.pixels().len());

        #[derive(Debug)]
        enum State {
            Raw { vals: Vec<u8> },
            Run { val: u8, len: u32 },
        }

        let mut states = Vec::<State>::new();

        for ch in rend.pixels().chunks_exact(8) {
            let mut cur_val = 0u8;
            for px in ch {
                cur_val <<= 1;
                let ttl = px.red() as u16 + px.green() as u16 + px.blue() as u16;
                let thresh = (256u16 * 3) / 2;
                if ttl >= thresh {
                    cur_val |= 0b1;
                }
            }
            if let Some(s) = states.last_mut() {
                match s {
                    State::Raw { vals } => {
                        if let Some(t) = vals.last() {
                            if *t == cur_val {
                                vals.pop();
                                if vals.is_empty() {
                                    states.pop();
                                }
                                states.push(State::Run {
                                    val: cur_val,
                                    len: 2,
                                });
                            } else {
                                vals.push(cur_val);
                            }
                        } else {
                            panic!();
                        }
                    }
                    State::Run { val, len } => {
                        if *val == cur_val {
                            *len += 1;
                        } else if *len == 1 {
                            let old = *val;
                            states.pop();
                            states.push(State::Raw {
                                vals: vec![old, cur_val],
                            });
                        } else {
                            states.push(State::Raw {
                                vals: vec![cur_val],
                            });
                        }
                    }
                }
            } else {
                states.push(State::Raw {
                    vals: vec![cur_val],
                });
            }
        }

        let client = connect("localhost:51837").await.unwrap();
        let mut offset = 0;
        for ch in states.chunks(64) {
            print!("Transfer chunk (offset: {offset})...");
            let mut out: Vec<u8> = vec![];
            let this_offset = offset;
            for s in ch {
                match s {
                    State::Run { val, len } => {
                        offset += len;
                        let now = postcard::to_stdvec(&(*len as i32)).unwrap();
                        out.extend_from_slice(&now);
                        out.push(*val);
                    }
                    State::Raw { vals } => {
                        offset += vals.len() as u32;
                        let now = postcard::to_stdvec(&-(vals.len() as i32)).unwrap();
                        out.extend_from_slice(&now);
                        out.extend_from_slice(vals);
                    },
                };
            }

            println!(" len: {}", out.len());

            client
                .proxy_endpoint::<SetDisplay>(
                    0x27927AE08C5C829B,
                    ctr(),
                    &DisplayCommand {
                        data: out,
                        offset: this_offset,
                    },
                )
                .await
                .unwrap()
                .unwrap();
            sleep(Duration::from_millis(10)).await;
        }
        sleep(Duration::from_millis(50)).await;

        let res = client
            .proxy_endpoint::<DrawDisplay>(0x27927AE08C5C829B, ctr(), &())
            .await
            .unwrap();

        if res.is_ok() {
            println!("Did draw");
        } else {
            println!("Didn't draw");
        }

        println!(":)");
    }
}
