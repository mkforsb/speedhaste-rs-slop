//! Dev tool: render a song from SPEEDH.JCL to raw s16 stereo.
//! usage: s3m_render SPEEDH.JCL SONG.S3M RATE SECONDS PANWIDTH OUT.raw
use speedhaste_core::data::{Data, Jcl};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = Data::new(Jcl::parse(std::fs::read(&a[1]).unwrap()).unwrap());
    let rate: u32 = a[3].parse().unwrap();
    let mut p = speedhaste_core::s3m::S3mPlayer::new(data.get(&a[2]).expect("no such song"), rate).expect("bad song");
    p.pan_width = a[5].parse().unwrap();
    let mut buf = vec![0f32; rate as usize * 2 * a[4].parse::<usize>().unwrap()];
    p.render(&mut buf, 1.0);
    let out: Vec<u8> = buf.iter().flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    std::fs::write(&a[6], out).unwrap();
}
