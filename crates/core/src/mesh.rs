//! I3D 3D meshes (wcgsl/object3d.c). The files are "B3D" binary dumps of
//! the Watcom in-memory `O3DM_TObject` (packed to 4 bytes, pointers stored as
//! offsets from the object start), followed by the material textures.

use crate::data::{Data, DataError, Reader, Result};

pub const MF_NOSHADE: u16 = 0x0001;
pub const MF_TRANS: u16 = 0x0002;
pub const MF_HOLES: u16 = 0x0004;
pub const MF_256: u16 = 0x0008;

pub const FF_FLAT: u32 = 0x0001;
pub const FF_NORDER: u32 = 0x0002;
pub const FF_VISIBLE: u32 = 0x0004;

/// Object flag: the first 4 groups of vertices are tires (game/flsprs.h).
pub const OF_HASTIRES: u16 = 0x0001;

pub struct Material {
    pub color: u8,
    pub flags: u16,
    pub ambient: i32,
    pub texture: Option<Vec<u8>>,
}

pub struct FaceVert {
    pub vert: usize,
    pub tx: i32,
    pub ty: i32,
}

pub struct Face {
    pub flags: u32,
    pub material: Option<usize>,
    pub verts: Vec<FaceVert>,
}

pub struct Mesh {
    pub flags: u16,
    pub verts: Vec<[i32; 3]>,
    pub faces: Vec<Face>,
    pub materials: Vec<Material>,
    pub sc: [i32; 3],
    pub dc: [i32; 3],
}

const VERT_SIZE: usize = 36;
const FACE_HDR: usize = 48;
const FACEVERT_SIZE: usize = 20;
const MAT_SIZE: usize = 28;

impl Mesh {
    pub fn load(d: &Data, name: &str) -> Result<Mesh> {
        Mesh::parse(name, d.load(name)?)
    }

    pub fn parse(name: &str, b: &[u8]) -> Result<Mesh> {
        let bad = |why| DataError::Format(name.to_string(), why);
        if b.len() < 8 || &b[..4] != b"B3D\0" {
            return Err(bad("not a binary B3D object"));
        }
        let size = u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize;
        let obj = b.get(8..8 + size).ok_or(bad("truncated"))?;
        let at = |o: usize| Reader { buf: obj, pos: o };
        let mut r = at(0);
        let mut w = || r.u16().ok_or(bad("header"));
        let (nverts, _nnorm, nfaces, _nfv, nmat, flags) = (w()?, w()?, w()?, w()?, w()?, w()?);
        let mut r = at(12);
        let mut p = || r.u32().map(|v| v as usize).ok_or(bad("header"));
        let (vofs, _nofs, fofs, mofs) = (p()?, p()?, p()?, p()?);
        let mut r = at(36);
        let mut sc = [0; 3];
        let mut dc = [0; 3];
        for v in sc.iter_mut().chain(dc.iter_mut()) {
            *v = r.i32().ok_or(bad("header"))?;
        }

        let verts = (0..nverts as usize)
            .map(|i| {
                let mut r = at(vofs + i * VERT_SIZE);
                Some([r.i32()?, r.i32()?, r.i32()?])
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(bad("vertices"))?;

        let mut materials = Vec::new();
        let mut tex_pos = 8 + size;
        for i in 0..nmat as usize {
            let mut r = at(mofs + i * MAT_SIZE);
            let color = r.u8().ok_or(bad("material"))?;
            r.pos += 1;
            let mflags = r.u16().ok_or(bad("material"))?;
            let ambient = r.i32().ok_or(bad("material"))?;
            r.pos = mofs + i * MAT_SIZE + 24;
            let has_tex = r.u32().ok_or(bad("material"))? != 0;
            let texture = if has_tex {
                let l = if mflags & MF_256 != 0 { 256 * 64 } else { 64 * 64 };
                let t = b.get(tex_pos..tex_pos + l).ok_or(bad("texture"))?.to_vec();
                tex_pos += l;
                Some(t)
            } else {
                None
            };
            materials.push(Material { color, flags: mflags, ambient, texture });
        }

        let mut faces = Vec::with_capacity(nfaces as usize);
        let mut fo = fofs;
        while faces.len() < nfaces as usize {
            let mut r = at(fo + 2);
            let nv = r.u16().ok_or(bad("face"))? as usize;
            let fflags = r.u32().ok_or(bad("face"))?;
            let mat = r.u32().ok_or(bad("face"))? as usize;
            let mut r = at(fo + 36);
            let front = r.u32().ok_or(bad("face"))? as usize;
            let material = if mat == 0 { None } else { Some((mat - mofs) / MAT_SIZE) };
            let fverts = (0..nv)
                .map(|k| {
                    let mut r = at(fo + FACE_HDR + k * FACEVERT_SIZE);
                    let v = (r.u32()? as usize).checked_sub(vofs)? / VERT_SIZE;
                    r.pos += 8;
                    Some(FaceVert { vert: v, tx: r.i32()?, ty: r.i32()? })
                })
                .collect::<Option<Vec<_>>>()
                .ok_or(bad("face vertices"))?;
            if fverts.iter().any(|v| v.vert >= verts.len()) {
                return Err(bad("face vertex index"));
            }
            faces.push(Face { flags: fflags, material, verts: fverts });
            if front == 0 {
                break;
            }
            fo = front;
        }
        Ok(Mesh { flags, verts, faces, materials, sc, dc })
    }
}
