//! Read original scene artwork without extracting archives or running scene scripts.
//! PKGV/TEXV layout references: https://github.com/notscuffed/repkg
//! Only a static source image is decoded; shader effects and animation are not rendered.
use std::{collections::{HashMap, HashSet}, fs::{self, File}, io::{BufReader, Cursor, Read, Seek, SeekFrom}, path::{Component, Path, PathBuf}};
use image::{DynamicImage, ImageReader};
use serde_json::Value;
mod composition;

const MAX_ASSET: usize = 128 * 1024 * 1024;
const MAX_PIXELS: u64 = 48 * 1024 * 1024;
const MAX_JSON: usize = 2 * 1024 * 1024;

fn u32_from(reader: &mut impl Read) -> Option<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes).ok()?;
    Some(u32::from_le_bytes(bytes))
}
fn safe_name(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    if name.is_empty() || name.contains(':') || name.starts_with('/') || name.contains('\0')
        || Path::new(&name).components().any(|part| !matches!(part, Component::Normal(_))) { return None; }
    Some(name)
}

struct Entry { offset: u64, size: usize }
struct Package { file: BufReader<File>, entries: HashMap<String, Entry> }
impl Package {
    fn open(path: &Path) -> Option<Self> {
        let mut file = BufReader::new(File::open(path).ok()?);
        let length = file.get_ref().metadata().ok()?.len();
        let size = u32_from(&mut file)? as usize;
        if !(8..=16).contains(&size) { return None; }
        let mut version = vec![0; size];
        file.read_exact(&mut version).ok()?;
        if !version.starts_with(b"PKGV") { return None; }
        let count = u32_from(&mut file)?;
        if count > 20_000 { return None; }
        let mut entries = HashMap::new();
        for _ in 0..count {
            let size = u32_from(&mut file)? as usize;
            if size == 0 || size > 4096 { return None; }
            let mut name = vec![0; size];
            file.read_exact(&mut name).ok()?;
            let name = safe_name(std::str::from_utf8(&name).ok()?)?.to_lowercase();
            let offset = u32_from(&mut file)? as u64;
            let size = u32_from(&mut file)? as usize;
            entries.insert(name, Entry { offset, size });
        }
        let base = file.stream_position().ok()?;
        for entry in entries.values_mut() {
            entry.offset = base.checked_add(entry.offset)?;
            if entry.offset.checked_add(entry.size as u64)? > length { return None; }
        }
        Some(Self { file, entries })
    }
}

fn local_files(root: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((folder, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(folder) else { continue; };
        for entry in entries.flatten() {
            if result.len() >= 4096 { return result; }
            let Ok(kind) = entry.file_type() else { continue; };
            if kind.is_symlink() { continue; }
            if kind.is_dir() && depth < 5 { pending.push((entry.path(), depth + 1)); }
            else if kind.is_file() { result.push(entry.path()); }
        }
    }
    result.sort();
    result
}

/// Includes source assets, so changing artwork invalidates covers even if preview.jpg is unchanged.
pub fn revision(project: &Path) -> String {
    use std::{collections::hash_map::DefaultHasher, hash::{Hash, Hasher}};
    let mut hash = DefaultHasher::new();
    "scene-hierarchy-v5".hash(&mut hash);
    if let Some(root) = project.parent() {
        for path in local_files(root) {
            if let Ok(metadata) = path.metadata() {
                path.hash(&mut hash);
                metadata.len().hash(&mut hash);
                metadata.modified().ok().hash(&mut hash);
            }
        }
    }
    format!("{:016x}", hash.finish())
}

struct Assets { root: PathBuf, package: Option<Package>, names: Vec<String>, scene: String }
impl Assets {
    fn open(project: &Path) -> Option<Self> {
        let root = project.parent()?.canonicalize().ok()?;
        let project_json: Value = serde_json::from_slice(&fs::read(project).ok()?).ok()?;
        let scene = project_json["file"].as_str().and_then(safe_name)
            .map(|name| Path::new(&name).with_extension("json").to_string_lossy().into_owned()).unwrap_or_else(|| "scene.json".into());
        let pkg = project_json["file"].as_str().and_then(safe_name)
            .map(|name| Path::new(&name).with_extension("pkg").to_string_lossy().into_owned()).unwrap_or_else(|| "scene.pkg".into());
        let package = root.join(pkg).canonicalize().ok().filter(|path| path.starts_with(&root))
            .and_then(|path| Package::open(&path));
        let mut names: Vec<_> = local_files(&root).iter().filter_map(|path| path.strip_prefix(&root).ok()
            .map(|path| path.to_string_lossy().replace('\\', "/"))).collect();
        if let Some(package) = &package { names.extend(package.entries.keys().cloned()); }
        names.sort();
        names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Some(Self { root, package, names, scene })
    }
    fn read(&mut self, name: &str, maximum: usize, prefix: bool) -> Option<Vec<u8>> {
        let name = safe_name(name)?;
        if let Ok(path) = self.root.join(&name).canonicalize() {
            if path.is_file() && path.starts_with(&self.root) {
                let size = fs::metadata(&path).ok()?.len();
                if !prefix && size > maximum as u64 { return None; }
                let mut bytes = Vec::new();
                File::open(path).ok()?.take(maximum as u64).read_to_end(&mut bytes).ok()?;
                return Some(bytes);
            }
        }
        let package = self.package.as_mut()?;
        let entry = package.entries.get(&name.to_lowercase())?;
        if !prefix && entry.size > maximum { return None; }
        let size = entry.size.min(maximum);
        package.file.seek(SeekFrom::Start(entry.offset)).ok()?;
        let mut data = vec![0; size];
        package.file.read_exact(&mut data).ok()?;
        Some(data)
    }
    fn json(&mut self, name: &str) -> Option<Value> { serde_json::from_slice(&self.read(name, MAX_JSON, false)?).ok() }
}

struct Bytes<'a> { data: &'a [u8], position: usize }
impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self { Self { data, position: 0 } }
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.position.checked_add(length)?;
        let bytes = self.data.get(self.position..end)?;
        self.position = end;
        Some(bytes)
    }
    fn number(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
    fn tag(&mut self) -> Option<&'a [u8]> { self.take(9) }
}

struct TextureInfo { format: u32, flags: u32, width: u32, height: u32, image_width: u32, image_height: u32 }
fn texture_info(data: &mut Bytes<'_>) -> Option<TextureInfo> {
    if data.tag()? != b"TEXV0005\0" || data.tag()? != b"TEXI0001\0" { return None; }
    let format = data.number()?;
    let flags = data.number()?;
    // Videos, normals and grayscale masks are not standalone artwork.
    if flags & 32 != 0 || ![0, 4, 6, 7].contains(&format) { return None; }
    let (width, height, image_width, image_height) = (data.number()?, data.number()?, data.number()?, data.number()?);
    data.number()?;
    if width == 0 || height == 0 || image_width == 0 || image_height == 0 || width > 16_384 || height > 16_384 { return None; }
    Some(TextureInfo { format, flags, width, height, image_width, image_height })
}

fn extended_length(data: &mut Bytes<'_>, base: usize) -> Option<usize> {
    let mut size = base;
    if base == 15 {
        loop { let next = data.take(1)?[0] as usize; size = size.checked_add(next)?; if next != 255 { break; } }
    }
    Some(size)
}
fn lz4_block(encoded: &[u8], expected: usize) -> Option<Vec<u8>> {
    if expected > MAX_ASSET * 2 { return None; }
    let mut data = Bytes::new(encoded);
    let mut output = Vec::with_capacity(expected);
    while data.position < encoded.len() {
        let token = data.take(1)?[0];
        let literals = extended_length(&mut data, (token >> 4) as usize)?;
        if output.len().checked_add(literals)? > expected { return None; }
        output.extend_from_slice(data.take(literals)?);
        if data.position == encoded.len() { break; }
        let offset = u16::from_le_bytes(data.take(2)?.try_into().ok()?) as usize;
        if offset == 0 || offset > output.len() { return None; }
        let count = extended_length(&mut data, (token & 15) as usize)?.checked_add(4)?;
        if output.len().checked_add(count)? > expected { return None; }
        let from = output.len() - offset;
        let end = output.len() + count;
        while output.len() < end {
            let chunk = (output.len() - from).min(end - output.len());
            output.extend_from_within(from..from + chunk);
        }
    }
    if output.len() == expected { Some(output) } else { None }
}

fn decode_image(bytes: &[u8]) -> Option<DynamicImage> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    reader.decode().ok()
}
fn dxt_image(data: &[u8], width: u32, height: u32, format: u32) -> Option<DynamicImage> {
    let (width, height) = (width.div_ceil(4) * 4, height.div_ceil(4) * 4);
    let required = width as usize / 4 * (height as usize / 4) * if format == 7 { 8 } else { 16 };
    if data.len() < required { return None; }
    // A DDS header lets image's maintained BC1/BC2/BC3 decoder handle the texture blocks.
    let mut dds = vec![0; 128];
    dds[..4].copy_from_slice(b"DDS ");
    for (at, value) in [(4,124u32),(8,0x81007),(12,height),(16,width),(20,required as u32),(76,32),(80,4),(108,0x1000)] {
        dds[at..at+4].copy_from_slice(&value.to_le_bytes());
    }
    dds[84..88].copy_from_slice(match format { 7 => b"DXT1", 6 => b"DXT3", _ => b"DXT5" });
    dds.extend_from_slice(&data[..required]);
    decode_image(&dds)
}

fn texture_image(bytes: &[u8]) -> Option<DynamicImage> {
    let mut data = Bytes::new(bytes);
    let info = texture_info(&mut data)?;
    let version = match data.tag()? { b"TEXB0001\0" => 1, b"TEXB0002\0" => 2, b"TEXB0003\0" => 3, b"TEXB0004\0" => 4, _ => return None };
    let images = data.number()?;
    if images == 0 || images > 100 { return None; }
    let embedded = if version >= 3 { data.number()? != u32::MAX } else { false };
    if version == 4 && data.number()? != 0 { return None; }
    let mut choices = Vec::new();
    for _ in 0..images {
        let mipmaps = data.number()?;
        if mipmaps == 0 || mipmaps > 32 { return None; }
        let mut chosen = None;
        for _ in 0..mipmaps {
            let (width, height) = (data.number()?, data.number()?);
            let (compressed, expanded) = if version >= 2 { (data.number()? != 0, data.number()? as usize) } else { (false, 0) };
            let size = data.number()? as usize;
            if size > MAX_ASSET || expanded > MAX_ASSET * 2 { return None; }
            let bytes = data.take(size)?;
            if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS { continue; }
            let visible_height = u64::from(height) * u64::from(info.image_height) / u64::from(info.height);
            if chosen.is_none() || visible_height >= 1240 { chosen = Some((width, height, compressed, expanded, bytes)); }
        }
        choices.push(chosen);
    }
    let frame = if info.flags & 4 != 0 {
        let version = match data.tag()? { b"TEXS0001\0" => 1, b"TEXS0002\0" => 2, b"TEXS0003\0" => 3, _ => return None };
        let frames = data.number()?;
        if frames == 0 || frames > 100_000 { return None; }
        if version == 3 { data.number()?; data.number()?; }
        let index = data.number()? as usize;
        data.number()?; // Frame duration.
        let mut values = [0.0; 6];
        for value in &mut values { let bits = data.number()?; *value = if version == 1 { bits as i32 as f32 } else { f32::from_bits(bits) }; }
        // Rotated sprite rectangles fall back to the author's preview rather than showing an atlas.
        if values.iter().any(|value| !value.is_finite()) || values[2] <= 0.0 || values[5] <= 0.0 || values[3] != 0.0 || values[4] != 0.0 { return None; }
        Some((index, values))
    } else { None };
    let index = frame.as_ref().map(|(index, _)| *index).unwrap_or(0);
    let (width, height, compressed, expanded, bytes) = choices.get(index).copied().flatten()?;
    let expanded_data;
    let bytes = if compressed { expanded_data = lz4_block(bytes, expanded)?; &expanded_data } else { bytes };
    let image = if embedded { decode_image(bytes)? } else if info.format == 0 {
        let needed = width as usize * height as usize * 4;
        DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, bytes.get(..needed)?.to_vec())?)
    } else { dxt_image(bytes, width, height, info.format)? };
    if let Some((_, rectangle)) = frame {
        let x = (rectangle[0] * image.width() as f32 / info.width as f32).round();
        let y = (rectangle[1] * image.height() as f32 / info.height as f32).round();
        let w = (rectangle[2] * image.width() as f32 / info.width as f32).round();
        let h = (rectangle[5] * image.height() as f32 / info.height as f32).round();
        if x < 0.0 || y < 0.0 || w < 1.0 || h < 1.0 || x + w > image.width() as f32 || y + h > image.height() as f32 { return None; }
        return Some(image.crop_imm(x as u32, y as u32, w as u32, h as u32));
    }
    if embedded { return Some(image); }
    let real_width = (u64::from(width) * u64::from(info.image_width) / u64::from(info.width)).clamp(1, u64::from(width)) as u32;
    let real_height = (u64::from(height) * u64::from(info.image_height) / u64::from(info.height)).clamp(1, u64::from(height)) as u32;
    Some(image.crop_imm(0, 0, real_width, real_height))
}

fn dimensions(bytes: &[u8], texture: bool) -> Option<(u32, u32)> {
    if texture { let info = texture_info(&mut Bytes::new(bytes))?; Some((info.image_width, info.image_height)) }
    else { ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok() }
}
fn signature(image: &DynamicImage) -> Vec<u8> {
    image.resize_exact(32, 18, image::imageops::FilterType::Triangle).to_rgb8().into_raw()
}

#[cfg(test)]
fn preview_crop(image: &DynamicImage, reference: &DynamicImage) -> DynamicImage {
    // Workshop previews are usually square center crops of widescreen artwork.
    // Stretching both into a signature compared different parts of the scene.
    let scale = (192.0 / reference.width().max(reference.height()) as f64).min(1.0);
    image.resize_to_fill((reference.width() as f64*scale).round().max(1.0) as u32, (reference.height() as f64*scale).round().max(1.0) as u32, image::imageops::FilterType::Triangle)
}

// Raw scene textures are often separate layers, atlases, or untransformed artwork.
// Only replace the author's composed preview when a source closely matches it.
fn matching_score(image: &DynamicImage, reference: &DynamicImage) -> Option<f64> {
    let sample = image.thumbnail(192,192);
    let thumbnail = sample.to_rgba8();
    if thumbnail.pixels().any(|pixel| pixel[3] < 250) { return None; }
    let reference_aspect = reference.width() as f64/reference.height() as f64;
    let reference = signature(reference);
    let mut best: Option<f64> = None;
    let mut compare = |sample: Vec<u8>| {
        let mean = sample.iter().zip(&reference).map(|(a,b)| (*a as f64-*b as f64).abs()).sum::<f64>() / sample.len() as f64;
        let correlation = spatial_correlation(&sample, &reference);
        // Allow lighting, fog and small text overlays only when spatial structure
        // agrees. A similarly coloured mask/background is not sufficient.
        if mean <= 30.0 && (correlation >= 0.80 || mean <= 15.0 && correlation >= 0.70 || mean <= 3.0) {
            let score = mean + (1.0-correlation)*10.0;
            if best.is_none_or(|current| score < current) { best = Some(score); }
        }
    };
    compare(signature(&sample));
    let width = sample.width() as f64;
    let height = sample.height() as f64;
    let base_width = width.min(height*reference_aspect);
    let base_height = height.min(width/reference_aspect);
    // Authors can zoom or offset square previews independently of their scene.
    // Search only small thumbnails; the returned cover retains the full HD frame.
    for zoom in [1.0,1.2,1.45,1.75,2.1,2.5] {
        let (w,h) = ((base_width/zoom).round().max(1.0) as u32,(base_height/zoom).round().max(1.0) as u32);
        for x in [0.0,0.25,0.5,0.75,1.0] {
            for y in [0.0,0.25,0.5,0.75,1.0] {
                let left = ((sample.width()-w) as f64*x).round() as u32;
                let top = ((sample.height()-h) as f64*y).round() as u32;
                compare(signature(&sample.crop_imm(left,top,w,h)));
            }
        }
    }
    best
}

fn spatial_correlation(a: &[u8], b: &[u8]) -> f64 {
    let count = (a.len()/3) as f64;
    let (mut covariance,mut va,mut vb) = (0.0,0.0,0.0);
    for channel in 0..3 {
        let ma = a.iter().skip(channel).step_by(3).map(|v| *v as f64).sum::<f64>()/count;
        let mb = b.iter().skip(channel).step_by(3).map(|v| *v as f64).sum::<f64>()/count;
        for (a,b) in a.iter().skip(channel).step_by(3).zip(b.iter().skip(channel).step_by(3)) {
            let (a,b) = (*a as f64-ma,*b as f64-mb);
            covariance += a*b; va += a*a; vb += b*b;
        }
    }
    if va < count*48.0 || vb < count*48.0 { return 0.0; }
    covariance/(va*vb).sqrt()
}

#[cfg(test)]
fn luminance_correlation(a: &[u8], b: &[u8]) -> f64 {
    let luma = |rgb: &[u8]| rgb[0] as f64*0.2126+rgb[1] as f64*0.7152+rgb[2] as f64*0.0722;
    let count = (a.len()/3) as f64;
    let ma = a.chunks_exact(3).map(luma).sum::<f64>()/count;
    let mb = b.chunks_exact(3).map(luma).sum::<f64>()/count;
    let (mut covariance,mut va,mut vb) = (0.0,0.0,0.0);
    for (a,b) in a.chunks_exact(3).zip(b.chunks_exact(3)) {
        let (a,b) = (luma(a)-ma,luma(b)-mb);
        covariance += a*b; va += a*a; vb += b*b;
    }
    if va < count*16.0 || vb < count*16.0 { return 0.0; }
    covariance/(va*vb).sqrt()
}

fn property(value: &Value) -> &Value { value.get("value").unwrap_or(value) }
fn vector(value: &Value, default: [f32; 3]) -> [f32; 3] {
    let Some(text) = property(value).as_str() else { return default; };
    let values: Vec<f32> = text.split_whitespace().filter_map(|part| part.parse().ok()).collect();
    let mut result = default;
    for (i, value) in values.into_iter().take(3).enumerate() { if value.is_finite() { result[i] = value; } }
    result
}
fn layer_texture(assets: &mut Assets, object: &Value) -> Option<String> {
    let model = assets.json(object["image"].as_str()?)?;
    let material = assets.json(model["material"].as_str()?)?;
    let name = material["passes"][0]["textures"][0].as_str()?;
    Some(format!("materials/{}.tex", name.trim_end_matches(".tex")))
}

#[cfg(test)]
fn composed_image(assets: &mut Assets, scene: &Value) -> Option<DynamicImage> {
    composition::render(assets,scene).map(|composition| composition.image)
}

/// Return a verified higher-resolution equivalent, otherwise use the author's preview.
pub fn scene_image(project: &Path, preview: Option<&Path>) -> Option<DynamicImage> {
    let mut data = Vec::new();
    File::open(preview?).ok()?.take(20 * 1024 * 1024).read_to_end(&mut data).ok()?;
    let reference = decode_image(&data)?;
    let mut assets = Assets::open(project)?;
    let scene = assets.json(&assets.scene.clone());
    let mut best: Option<(f64, DynamicImage)> = None;
    let mut scene_composition = None;
    if let Some(composition) = scene.as_ref().and_then(|scene| composition::render(&mut assets, scene)) {
        let composed = composition.image;
        let reference_colour = signature(&reference).chunks_exact(3).map(|rgb| (rgb[0] as f32-rgb[1] as f32).abs()+(rgb[1] as f32-rgb[2] as f32).abs()).sum::<f32>()/576.0;
        let variants = if reference_colour < 3.0 { vec![DynamicImage::ImageRgb8(composed.grayscale().to_rgb8())] } else { vec![composed] };
        for image in variants {
            // The scene's own resolved HD image layers are authoritative. The
            // Workshop thumbnail can be a collage, contain promotional text,
            // or use different filters and should not veto a complete scene.
            if composition.reliable { scene_composition = Some(image.clone()); }
            if let Some(score) = matching_score(&image, &reference) {
                if best.as_ref().is_none_or(|(current,_)| score < *current) { best = Some((score,image)); }
            }
        }
    }
    let mut referenced = HashSet::new();
    if let Some(objects) = scene.as_ref().and_then(|scene| scene["objects"].as_array()) {
        for object in objects.iter().take(2048) {
            let Some(model) = object["image"].as_str() else { continue; };
            let Some(model) = assets.json(model) else { continue; };
            let Some(material) = model["material"].as_str() else { continue; };
            let Some(material) = assets.json(material) else { continue; };
            if let Some(passes) = material["passes"].as_array() {
                for pass in passes {
                    if let Some(name) = pass["textures"].get(0).and_then(Value::as_str) {
                        referenced.insert(format!("materials/{}.tex", name.trim_end_matches(".tex")).to_lowercase());
                    }
                }
            }
        }
    }
    let mut candidates = Vec::new();
    for name in assets.names.clone() {
        let lower = name.to_lowercase();
        let texture = lower.ends_with(".tex");
        if !texture && ![".jpg", ".jpeg", ".png", ".webp"].iter().any(|ext| lower.ends_with(ext)) { continue; }
        if lower.contains("/masks/") || lower.contains("/effects/") || lower.contains("preview.")
            || lower.contains("normal") || lower.contains("depth") { continue; }
        let Some(header) = assets.read(&name, 8192, true) else { continue; };
        let Some((width, height)) = dimensions(&header, texture) else { continue; };
        let area = u64::from(width) * u64::from(height);
        if width < 256 || height < 256 || area > MAX_PIXELS { continue; }
        let rank = area as f64 * if referenced.contains(&lower) { 2.0 } else { 1.0 };
        candidates.push((rank, name, texture));
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, name, texture) in candidates.into_iter().take(8) {
        let Some(bytes) = assets.read(&name, MAX_ASSET, false) else { continue; };
        let decoded = if texture { texture_image(&bytes) } else { decode_image(&bytes) };
        let Some(image) = decoded else { continue; };
        let Some(similarity) = matching_score(&image, &reference) else { continue; };
        // Equal-looking originals beat tiny source images; comparison uses the entire image, before portrait cropping.
        let score = similarity - (u64::from(image.width()) * u64::from(image.height())).min(16_000_000) as f64 / 16_000_000.0;
        if best.as_ref().is_none_or(|(best_score, _)| score < *best_score) { best = Some((score, image)); }
    }
    best.map(|(_, image)| image).or(scene_composition)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "read-only audit of the selected installed playlist; optional image output"]
    fn audit_installed_playlist_covers() {
        let executable = Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\wallpaper_engine\wallpaper64.exe");
        let config = crate::engine_config::EngineConfig::read(executable).unwrap();
        let name = std::env::var("WALLFLOW_AUDIT_PLAYLIST").unwrap_or_else(|_| "Showcase".into());
        let playlist = config.playlists.iter().find(|p| p.name==name).unwrap();
        let output = std::env::var_os("WALLFLOW_COVER_DIAGNOSTICS").map(PathBuf::from);
        if let Some(output) = &output { fs::create_dir_all(output).unwrap(); }
        for file in &playlist.items {
            let Some(root) = Path::new(file).parent() else { continue; };
            let project = root.join("project.json");
            let Ok(bytes) = fs::read(&project) else { continue; };
            let metadata: Value = serde_json::from_slice(&bytes).unwrap();
            if !metadata["type"].as_str().is_some_and(|kind| kind.eq_ignore_ascii_case("scene")) { continue; }
            let id = root.file_name().unwrap().to_string_lossy();
            let reference = decode_image(&fs::read(root.join(metadata["preview"].as_str().unwrap())).unwrap()).unwrap();
            let result = scene_image(&project,Some(&root.join(metadata["preview"].as_str().unwrap())));
            eprintln!("AUDIT {id} preview {}x{} -> {:?}",reference.width(),reference.height(),result.as_ref().map(|image| (image.width(),image.height())));
            if let Some(output) = &output { reference.to_rgb8().save(output.join(format!("{id}-preview.jpg"))).unwrap(); }
            if let Some(image) = result { if let Some(output) = &output { image.thumbnail(960,540).to_rgb8().save(output.join(format!("{id}-result.jpg"))).unwrap(); } continue; }
            let mut assets = Assets::open(&project).unwrap();
            if let Some(output) = &output {
                if let Some(scene) = assets.json(&assets.scene.clone()) {
                    if let Some(image) = composed_image(&mut assets,&scene) { image.thumbnail(960,540).save(output.join(format!("{id}-composed.png"))).unwrap(); }
                }
            }
            let mut names = Vec::new();
            for name in assets.names.clone().into_iter().filter(|name| name.ends_with(".tex")) {
                let Some(header) = assets.read(&name,8192,true) else { continue; };
                if let Some((w,h)) = dimensions(&header,true) { names.push((w as u64*h as u64,name)); }
            }
            names.sort_by(|a,b| b.0.cmp(&a.0));
            for (i,(_,name)) in names.into_iter().take(5).enumerate() {
                let image = assets.read(&name,MAX_ASSET,false).and_then(|bytes| texture_image(&bytes));
                eprintln!("  {id} {i} {name} decoded={:?} match={:?}",image.as_ref().map(|im| (im.width(),im.height())),image.as_ref().and_then(|im| matching_score(im,&reference)));
                if let (Some(output),Some(image)) = (&output,image) { image.thumbnail(720,480).save(output.join(format!("{id}-candidate-{i}.png"))).unwrap(); }
            }
        }
    }
    #[test]
    #[ignore = "manual read-only scene quality diagnostics; optional output directory"]
    fn scene_quality_diagnostics() {
        let output = std::env::var_os("WALLFLOW_COVER_DIAGNOSTICS").map(PathBuf::from);
        for id in ["3653567054", "2984126406", "3792108790", "1888322953"] {
            let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960").join(id);
            let metadata: Value = serde_json::from_slice(&fs::read(root.join("project.json")).unwrap()).unwrap();
            let reference = decode_image(&fs::read(root.join(metadata["preview"].as_str().unwrap())).unwrap()).unwrap();
            eprintln!("{id} preview {}x{}", reference.width(), reference.height());
            if let Some(output) = &output { fs::create_dir_all(output).unwrap(); reference.to_rgb8().save(output.join(format!("{id}-preview.jpg"))).unwrap(); }
            let mut assets = Assets::open(&root.join("project.json")).unwrap();
            let scene = assets.json(&assets.scene.clone()).unwrap();
            if let Some(composed) = composed_image(&mut assets, &scene) {
                for (kind,image) in [("composed",composed.clone()),("gray",DynamicImage::ImageRgb8(composed.grayscale().to_rgb8()))] {
                    let mean = signature(&preview_crop(&image,&reference)).iter().zip(signature(&reference)).map(|(a,b)| (*a as f64-b as f64).abs()).sum::<f64>()/1728.0;
                    let correlation = luminance_correlation(&signature(&preview_crop(&image,&reference)),&signature(&reference));
                    eprintln!("{id} {kind} score {:?}, mean {mean:.1}, correlation {correlation:.2}",matching_score(&image,&reference));
                    if let Some(output) = &output { image.thumbnail(960,540).to_rgb8().save(output.join(format!("{id}-{kind}.jpg"))).unwrap(); }
                }
            }
            let mut candidates = Vec::new();
            for name in assets.names.clone().into_iter().filter(|name| name.ends_with(".tex")) {
                let Some(header) = assets.read(&name, 8192, true) else { continue; };
                let Some((w,h)) = dimensions(&header,true) else { continue; };
                candidates.push((w as u64*h as u64,name));
            }
            candidates.sort_by(|a,b| b.0.cmp(&a.0));
            for (index,(_,name)) in candidates.into_iter().take(4).enumerate() {
                let Some(image) = assets.read(&name,MAX_ASSET,false).and_then(|bytes| texture_image(&bytes)) else { continue; };
                let error = |image: &DynamicImage| signature(&preview_crop(image,&reference)).iter().zip(signature(&reference)).map(|(a,b)| (*a as f64-b as f64).abs()).sum::<f64>()/1728.0;
                eprintln!("{id} {name} {}x{} error {:.1} flipped {:.1} rotated {:.1}",image.width(),image.height(),error(&image),error(&image.flipv()),error(&image.rotate180()));
                if let Some(output) = &output { image.thumbnail(960,540).to_rgb8().save(output.join(format!("{id}-{index}.jpg"))).unwrap(); }
            }
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("wallflow-covers-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let root = std::env::temp_dir().canonicalize().unwrap();
            if self.0.parent() == Some(root.as_path()) && self.0.file_name().unwrap().to_string_lossy().starts_with("wallflow-covers-") { let _ = fs::remove_dir_all(&self.0); }
        }
    }
    #[test]
    fn lz4_handles_overlap_and_rejects_corruption_and_oversized_output() {
        assert_eq!(lz4_block(&[0x32,b'a',b'b',b'c',3,0], 9).unwrap(), b"abcabcabc");
        assert!(lz4_block(&[0x00,0,0], 4).is_none());
        assert!(lz4_block(&[0xf0], 20).is_none());
        assert!(lz4_block(&[0x10,b'a'], MAX_ASSET * 2 + 1).is_none());
        assert!(lz4_block(&[0x10,b'a'], 2).is_none());
    }
    #[test]
    fn archive_names_cannot_escape_the_project() {
        for name in ["../secret.png", "materials/../../secret.png", "C:/secret.png", "/secret", "\\\\server\\secret"] { assert!(safe_name(name).is_none()); }
        assert_eq!(safe_name("materials\\art.tex").unwrap(), "materials/art.tex");
    }
    fn texture(width: u32, height: u32, visible_width: u32, visible_height: u32, format: u32, pixels: &[u8]) -> Vec<u8> {
        let mut data = b"TEXV0005\0TEXI0001\0".to_vec();
        for number in [format,2,width,height,visible_width,visible_height,0] { data.extend_from_slice(&number.to_le_bytes()); }
        data.extend_from_slice(b"TEXB0002\0");
        for number in [1,1,width,height,0,pixels.len() as u32,pixels.len() as u32] { data.extend_from_slice(&number.to_le_bytes()); }
        data.extend_from_slice(pixels);
        data
    }
    #[test]
    fn source_texture_is_cropped_from_padding_and_preserves_real_pixels() {
        let pixels = [40,70,90,255].repeat(16);
        let image = texture_image(&texture(4,4,3,2,0,&pixels)).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (3,2));
        assert_eq!(image.get_pixel(2,1).0, [40,70,90,255]);
        assert!(texture_image(&texture(4,4,3,2,0,&pixels[..8])).is_none());
    }
    #[test]
    fn original_dxt1_texture_is_decoded_without_external_tools() {
        let block = [0x00,0xf8,0,0,0,0,0,0];
        let image = texture_image(&texture(4,4,4,4,7,&block)).unwrap().to_rgb8();
        assert_eq!(image.get_pixel(1,1).0, [255,0,0]);
    }
    #[test]
    fn animated_texture_uses_one_frame_instead_of_showing_the_entire_sprite_atlas() {
        let mut pixels = [40,70,90,255].repeat(16);
        pixels[24..28].copy_from_slice(&[0,255,0,255]);
        let mut data = texture(4,4,4,4,0,&pixels);
        data[22..26].copy_from_slice(&4u32.to_le_bytes());
        data.extend_from_slice(b"TEXS0003\0");
        for value in [1u32,2,2,0,0.1f32.to_bits()] { data.extend_from_slice(&value.to_le_bytes()); }
        for value in [2.0f32,1.0,2.0,0.0,0.0,2.0] { data.extend_from_slice(&value.to_bits().to_le_bytes()); }
        let image = texture_image(&data).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (2,2));
        assert_eq!(image.get_pixel(0,0).0, [0,255,0,255]);
    }
    #[test]
    fn package_index_rejects_out_of_bounds_entries() {
        let fixture = Fixture::new();
        let path = fixture.0.join("scene.pkg");
        let name = b"materials/art.tex";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&8u32.to_le_bytes()); bytes.extend_from_slice(b"PKGV0024");
        bytes.extend_from_slice(&1u32.to_le_bytes()); bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(name); bytes.extend_from_slice(&0u32.to_le_bytes()); bytes.extend_from_slice(&3u32.to_le_bytes());
        let offset_at = bytes.len() - 8;
        bytes.extend_from_slice(b"abc"); fs::write(&path, &bytes).unwrap();
        assert_eq!(Package::open(&path).unwrap().entries["materials/art.tex"].size, 3);
        bytes[offset_at..offset_at+4].copy_from_slice(&u32::MAX.to_le_bytes()); fs::write(&path, bytes).unwrap();
        assert!(Package::open(&path).is_none());
    }
    #[test]
    fn local_artwork_is_used_and_changing_it_invalidates_the_cover_revision() {
        let fixture = Fixture::new();
        let project = fixture.0.join("project.json");
        fs::write(&project, r#"{"file":"custom.json","type":"scene"}"#).unwrap();
        fs::write(fixture.0.join("custom.json"), r#"{"objects":[]}"#).unwrap();
        let original = fixture.0.join("art.png");
        image::RgbImage::from_pixel(900,600,image::Rgb([40,70,90])).save(&original).unwrap();
        let preview = fixture.0.join("preview.png");
        image::RgbImage::from_pixel(90,60,image::Rgb([40,70,90])).save(&preview).unwrap();
        let before = revision(&project);
        let image = scene_image(&project, Some(&preview)).unwrap();
        assert_eq!((image.width(), image.height()), (900,600));
        image::RgbImage::from_pixel(901,601,image::Rgb([90,70,40])).save(&original).unwrap();
        assert_ne!(revision(&project), before);
    }
    #[test]
    fn unrelated_layers_and_unverified_artwork_fall_back_to_author_preview() {
        let fixture = Fixture::new();
        let project = fixture.0.join("project.json");
        fs::write(&project, r#"{"file":"scene.json","type":"scene"}"#).unwrap();
        let preview = fixture.0.join("preview.png");
        image::RgbImage::from_pixel(90,60,image::Rgb([40,70,90])).save(&preview).unwrap();
        image::RgbImage::from_pixel(900,600,image::Rgb([190,10,10])).save(fixture.0.join("layer.png")).unwrap();
        assert!(scene_image(&project, Some(&preview)).is_none());
        assert!(scene_image(&project, None).is_none());
        let transparent = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(900,600,image::Rgba([40,70,90,100])));
        transparent.save(fixture.0.join("layer.png")).unwrap();
        assert!(scene_image(&project, Some(&preview)).is_none());
    }
    #[test]
    fn matching_requires_the_same_composition_not_only_similar_average_colours() {
        let art = image::RgbImage::from_fn(900,600,|x,y| image::Rgb([(x * 255 / 900) as u8, (y * 255 / 600) as u8, 100]));
        let art = DynamicImage::ImageRgb8(art);
        let preview = art.thumbnail(90,60);
        assert!(matching_score(&art, &preview).is_some());
        assert!(matching_score(&art.fliph(), &preview).is_none());
        assert!(matching_score(&art.rotate180(), &preview).is_none());
    }
    #[test]
    fn square_zoomed_previews_can_identify_their_original_panorama() {
        let art = DynamicImage::ImageRgb8(image::RgbImage::from_fn(900,600,|x,y| image::Rgb([(x*255/900) as u8,(y*255/600) as u8,((x/9+y/6)%256) as u8])));
        let preview = art.crop_imm(225,75,450,450).thumbnail(90,90);
        assert!(matching_score(&art,&preview).is_some());
    }
    #[test]
    fn static_scene_composition_honours_visibility_position_colour_and_alpha() {
        let fixture = Fixture::new();
        let project = fixture.0.join("project.json");
        fs::write(&project,r#"{"file":"custom.json"}"#).unwrap();
        fs::create_dir(fixture.0.join("models")).unwrap();
        fs::create_dir(fixture.0.join("materials")).unwrap();
        fs::write(fixture.0.join("models/layer.json"),r#"{"material":"materials/layer.json"}"#).unwrap();
        fs::write(fixture.0.join("materials/layer.json"),r#"{"passes":[{"textures":["layer"]}]}"#).unwrap();
        fs::write(fixture.0.join("materials/layer.tex"),texture(4,4,4,4,0,&[255,0,0,255].repeat(16))).unwrap();
        let scene = serde_json::json!({"general":{"orthogonalprojection":{"width":200,"height":100},"clearcolor":"0 0 1"},"objects":[
            {"image":"models/layer.json","origin":"50 50 0","size":"100 100","alpha":{"value":0.5}},
            {"image":"models/layer.json","origin":"150 50 0","size":"100 100","visible":{"value":false}}
        ]});
        let mut assets = Assets::open(&project).unwrap();
        let composed = composed_image(&mut assets,&scene).unwrap().to_rgb8();
        assert_eq!(composed.dimensions(),(200,100));
        let left = composed.get_pixel(50,50);
        assert!((left[0] as i32-128).abs()<=1 && (left[2] as i32-127).abs()<=1);
        assert_eq!(composed.get_pixel(150,50).0,[0,0,255]);
    }
    #[test]
    #[ignore = "requires the installed Workshop library"]
    fn installed_square_previews_resolve_to_hd_artwork() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960");
        for id in ["2984126406","3792108790","1888322953"] {
            let project = root.join(id).join("project.json");
            let metadata: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
            let preview = root.join(id).join(metadata["preview"].as_str().unwrap());
            let image = scene_image(&project,Some(&preview)).expect(id);
            assert!(image.width()>=1900 && image.height()>=1080,"{id}: {}x{}",image.width(),image.height());
        }
    }
    #[test]
    #[ignore = "requires the reported local Workshop scene"]
    fn reported_layered_scene_has_a_verified_high_resolution_composition() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960\3653567054");
        assert!(root.join("preview.gif").is_file());
        let image = scene_image(&root.join("project.json"), Some(&root.join("preview.gif"))).expect("Reconstruct the scene instead of enlarging its 192px preview");
        assert!(image.width() >= 1920 && image.height() >= 1080);
        if let Some(output) = std::env::var_os("WALLFLOW_COVER_DIAGNOSTICS") {
            let output = PathBuf::from(output); fs::create_dir_all(&output).unwrap();
            image.to_rgb8().save(output.join("3653567054-hd.jpg")).unwrap();
        }
    }
    #[test]
    #[ignore = "requires the user's newly reported installed scene"]
    fn grouped_empress_scene_uses_hd_instead_of_its_160px_gif() {
        let root=Path::new(r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960\3810943704");
        let image=scene_image(&root.join("project.json"),Some(&root.join("preview.gif"))).expect("The character is a child of a transform group");
        assert!(image.width()>=1920 && image.height()>=1080);
        if let Some(output)=std::env::var_os("WALLFLOW_COVER_DIAGNOSTICS") {
            let output=PathBuf::from(output);fs::create_dir_all(&output).unwrap();
            image.thumbnail(1280,720).to_rgb8().save(output.join("3810943704-result.jpg")).unwrap();
        }
    }
    #[test]
    #[ignore = "requires the local Wallpaper Engine library"]
    fn installed_scene_textures_decode_at_original_resolution() {
        let root = Path::new(r"C:\Program Files (x86)\Steam\steamapps\workshop\content\431960");
        for id in ["2984126406", "3792108790", "1888322953"] {
            let project = root.join(id).join("project.json");
            let mut assets = Assets::open(&project).unwrap();
            let mut found = false;
            for name in assets.names.clone().into_iter().filter(|name| name.ends_with(".tex")) {
                let Some(header) = assets.read(&name, 8192, true) else { continue; };
                let Some((width, height)) = dimensions(&header, true) else { continue; };
                if width < 1900 || height < 1080 { continue; }
                let Some(bytes) = assets.read(&name, MAX_ASSET, false) else { continue; };
                let Some(image) = texture_image(&bytes) else { continue; };
                if image.width() >= 1900 && image.height() >= 1080 { found = true; break; }
            }
            assert!(found, "{id}: original textures must remain decodable even when they are unsuitable as covers");
        }
        let animated = root.join("3400828344");
        let mut assets = Assets::open(&animated.join("project.json")).unwrap();
        let found = assets.names.clone().into_iter().filter(|name| [".tex", ".png", ".jpg", ".jpeg", ".webp"].iter().any(|ext| name.ends_with(ext))).any(|name| {
            assets.read(&name, MAX_ASSET, false).and_then(|bytes| if name.ends_with(".tex") { texture_image(&bytes) } else { decode_image(&bytes) })
                .is_some_and(|image| (image.width(), image.height()) == (426,233))
        });
        assert!(found, "Animated textures must decode a single frame rather than the atlas");
    }
}
