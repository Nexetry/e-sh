fn main() {
    println!("cargo:rerun-if-changed=assets/icon-1024.png");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        use std::{fs, io::BufWriter, path::PathBuf};

        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let png_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/icon-1024.png");
        let png_bytes = fs::read(&png_path).expect("read assets/icon-1024.png");
        let img = image::load_from_memory_with_format(&png_bytes, image::ImageFormat::Png)
            .expect("decode icon-1024.png");

        let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
        for size in [16u32, 32, 48, 64, 128, 256] {
            let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
            let rgba = resized.to_rgba8();
            let icon_image = ico::IconImage::from_rgba_data(size, size, rgba.into_raw());
            let entry = ico::IconDirEntry::encode(&icon_image).expect("encode ico entry");
            icon_dir.add_entry(entry);
        }

        let ico_path = out_dir.join("e-sh.ico");
        let file = fs::File::create(&ico_path).expect("create e-sh.ico");
        icon_dir.write(BufWriter::new(file)).expect("write e-sh.ico");

        let rc_path = out_dir.join("e-sh.rc");
        let rc_contents = format!(
            "1 ICON \"{}\"\n",
            ico_path.display().to_string().replace('\\', "\\\\")
        );
        fs::write(&rc_path, rc_contents).expect("write e-sh.rc");

        embed_resource::compile(&rc_path, embed_resource::NONE)
            .manifest_optional()
            .expect("embed-resource compile");
    }
}
