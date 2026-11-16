use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub fn get_part_path(full_path: &PathBuf) -> PathBuf {
    let mut part = full_path.clone();
    let new_ext = match full_path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{}.rustdm.part", ext),
        None => String::from("rustdm.part"),
    };
    part.set_extension(new_ext);
    part
}

pub fn pick_folder_via_explorer(current_dir: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().set_title("Выберите папку для сохранения файла");
    if PathBuf::from(current_dir).exists() {
        dialog = dialog.set_directory(current_dir);
    }
    dialog.pick_folder().map(|p| p.display().to_string())
}

pub fn open_in_windows_explorer(folder: &str, file_path: &PathBuf) {
    let part_path = get_part_path(file_path);
    if file_path.exists() {
        let _ = Command::new("explorer")
            .arg(format!("/select,{}", file_path.display()))
            .spawn();
    } else if part_path.exists() {
        let _ = Command::new("explorer")
            .arg(format!("/select,{}", part_path.display()))
            .spawn();
    } else {
        let _ = fs::create_dir_all(folder);
        let _ = Command::new("explorer").arg(folder).spawn();
    }
}

pub fn purge_partial_files(full_path: &PathBuf) {
    let part_path = get_part_path(full_path);
    if part_path.exists() {
        let _ = fs::remove_file(&part_path);
    }
    if full_path.exists() {
        let _ = fs::remove_file(full_path);
    }
}

pub fn exists_on_disk(full_path: &PathBuf, is_finished: bool) -> bool {
    if is_finished {
        full_path.exists()
    } else {
        let part_path = get_part_path(full_path);
        part_path.exists() || full_path.exists()
    }
}