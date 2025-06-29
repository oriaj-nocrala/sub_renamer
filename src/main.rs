use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{command, Parser};
use regex::Regex;
use walkdir::WalkDir;

/// Renombra subtítulos para que coincidan con los nombres de sus archivos de video correspondientes.
#[derive(Parser, Debug)]
#[command(
    author = "Jairo Alarcón <jairo.alarconr@gmail.com>",
    version = "1.0.0", 
    about = "Herramienta para renombrar subtítulos basándose en archivos de video",
    long_about = "Esta herramienta busca archivos de subtítulos y videos, extrae identificadores usando regex y renombra los subtítulos para que coincidan con sus videos correspondientes."
)]
struct Args {
    /// Regex para capturar el ID de episodio desde archivos de subtítulos
    #[arg(
        long,
        help = "Patrón regex para extraer ID de episodio de subtítulos (ej: 'S(\\d{2})E(\\d{2})')"
    )]
    srt_regex: Option<String>,

    /// Regex para capturar el ID de episodio desde archivos de video
    #[arg(
        long,
        help = "Patrón regex para extraer ID de episodio de videos (ej: 'S(\\d{2})E(\\d{2})')"
    )]
    mkv_regex: Option<String>,

    /// Extensiones de subtítulos (separadas por coma)
    #[arg(
        long,
        default_value = "srt",
        help = "Extensiones de subtítulos separadas por coma (ej: srt,ass,vtt)"
    )]
    srt_ext: String,

    /// Extensiones de video (separadas por coma)
    #[arg(
        long,
        default_value = "mkv",
        help = "Extensiones de video separadas por coma (ej: mkv,mp4,avi)"
    )]
    video_ext: String,

    /// Directorio de trabajo (por defecto el actual)
    #[arg(
        short,
        long,
        default_value = ".",
        help = "Directorio donde buscar archivos"
    )]
    directory: PathBuf,

    /// Buscar en subdirectorios
    #[arg(short, long, help = "Buscar recursivamente en subdirectorios")]
    recursive: bool,

    /// Modo de prueba (no renombra archivos realmente)
    #[arg(
        long,
        help = "Modo de prueba: muestra qué archivos se renombrarían sin hacerlo"
    )]
    dry_run: bool,

    /// Modo silencioso (solo errores)
    #[arg(short, long, help = "Modo silencioso: solo muestra errores")]
    quiet: bool,

    /// Modo verbose (información detallada)
    #[arg(short, long, help = "Modo verbose: muestra información detallada")]
    verbose: bool,
}

#[derive(Debug, Clone)]
struct FileInfo {
    path: PathBuf,
    episode_id: String,
    extension: String,
}

#[derive(Debug)]
struct RenameOperation {
    from: PathBuf,
    to: PathBuf,
    episode_id: String,
}

struct SubtitleRenamer {
    args: Args,
    srt_regex: Regex,
    mkv_regex: Regex,
    srt_extensions: Vec<String>,
    video_extensions: Vec<String>,
}

impl SubtitleRenamer {
    fn new(args: Args) -> Result<Self> {
        // Validar que al menos un regex esté presente
        if args.srt_regex.is_none() && args.mkv_regex.is_none() {
            anyhow::bail!("❌ Debes proporcionar al menos un regex (--srt-regex o --mkv-regex)");
        }

        // Usar el regex disponible como fallback
        let srt_re_str = args.srt_regex.as_ref()
            .or(args.mkv_regex.as_ref())
            .unwrap();
        let mkv_re_str = args.mkv_regex.as_ref()
            .or(args.srt_regex.as_ref())
            .unwrap();

        let srt_regex = Regex::new(srt_re_str)
            .with_context(|| format!("Regex inválido para subtítulos: {}", srt_re_str))?;
        
        let mkv_regex = Regex::new(mkv_re_str)
            .with_context(|| format!("Regex inválido para videos: {}", mkv_re_str))?;

        let srt_extensions = Self::parse_extensions(&args.srt_ext);
        let video_extensions = Self::parse_extensions(&args.video_ext);

        // Validar que el directorio existe
        if !args.directory.exists() {
            anyhow::bail!("❌ El directorio {:?} no existe", args.directory);
        }

        Ok(Self {
            args,
            srt_regex,
            mkv_regex,
            srt_extensions,
            video_extensions,
        })
    }

    fn parse_extensions(ext_str: &str) -> Vec<String> {
        ext_str
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    fn get_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        
        if self.args.recursive {
            for entry in WalkDir::new(&self.args.directory) {
                match entry {
                    Ok(e) if e.file_type().is_file() => {
                        files.push(e.path().to_path_buf());
                    }
                    Ok(_) => {} // Ignorar directorios
                    Err(e) => {
                        if !self.args.quiet {
                            eprintln!("⚠️ Error accediendo a archivo: {}", e);
                        }
                    }
                }
            }
        } else {
            let dir_entries = fs::read_dir(&self.args.directory)
                .with_context(|| format!("No se pudo leer el directorio {:?}", self.args.directory))?;
            
            for entry in dir_entries {
                match entry {
                    Ok(e) if e.file_type().map_or(false, |ft| ft.is_file()) => {
                        files.push(e.path());
                    }
                    Ok(_) => {} // Ignorar directorios
                    Err(e) => {
                        if !self.args.quiet {
                            eprintln!("⚠️ Error accediendo a archivo: {}", e);
                        }
                    }
                }
            }
        }

        Ok(files)
    }

    fn extract_episode_id(&self, path: &Path, is_subtitle: bool) -> Option<String> {
        let file_name = path.file_name()?.to_str()?;
        let regex = if is_subtitle { &self.srt_regex } else { &self.mkv_regex };
        
        regex.captures(file_name)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().to_string())
    }

    fn categorize_files(&self) -> Result<(Vec<FileInfo>, Vec<FileInfo>)> {
        let mut subtitles = Vec::new();
        let mut videos = Vec::new();

        let files = self.get_files()?;

        for path in files {
            if let Some(extension) = path.extension()
                .and_then(OsStr::to_str)
                .map(str::to_lowercase)
            {
                if self.srt_extensions.contains(&extension) {
                    if let Some(episode_id) = self.extract_episode_id(&path, true) {
                        subtitles.push(FileInfo {
                            path,
                            episode_id,
                            extension,
                        });
                    }
                } else if self.video_extensions.contains(&extension) {
                    if let Some(episode_id) = self.extract_episode_id(&path, false) {
                        videos.push(FileInfo {
                            path,
                            episode_id,
                            extension,
                        });
                    }
                }
            }
        }

        if self.args.verbose {
            println!("📊 Encontrados {} subtítulos y {} videos", subtitles.len(), videos.len());
        }

        Ok((subtitles, videos))
    }

    fn plan_renames(&self, subtitles: Vec<FileInfo>, videos: Vec<FileInfo>) -> Vec<RenameOperation> {
        let video_map: HashMap<String, &FileInfo> = videos
            .iter()
            .map(|v| (v.episode_id.clone(), v))
            .collect();

        let mut operations = Vec::new();

        for subtitle in &subtitles {
            if let Some(video) = video_map.get(&subtitle.episode_id) {
                let video_stem = video.path.file_stem()
                    .and_then(OsStr::to_str)
                    .unwrap_or("unknown");
                
                let new_name = format!("{}.{}", video_stem, subtitle.extension);
                let new_path = subtitle.path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(&new_name);

                // Evitar renombrar a sí mismo
                if subtitle.path != new_path {
                    operations.push(RenameOperation {
                        from: subtitle.path.clone(),
                        to: new_path,
                        episode_id: subtitle.episode_id.clone(),
                    });
                }
            } else if !self.args.quiet {
                println!(
                    "⚠️ No se encontró video para episodio '{}' (subtítulo: {:?})",
                    subtitle.episode_id,
                    subtitle.path.file_name().unwrap_or_default()
                );
            }
        }

        operations
    }

    fn execute_renames(&self, operations: Vec<RenameOperation>) -> Result<()> {
        if operations.is_empty() {
            if !self.args.quiet {
                println!("ℹ️ No hay archivos para renombrar");
            }
            return Ok(());
        }

        let mut success_count = 0;
        let mut error_count = 0;

        for op in operations {
            // Verificar si el archivo de destino ya existe
            if op.to.exists() && op.from != op.to {
                if !self.args.quiet {
                    println!(
                        "⚠️ El archivo de destino ya existe: {:?} (episodio: {})",
                        op.to.file_name().unwrap_or_default(),
                        op.episode_id
                    );
                }
                continue;
            }

            if self.args.dry_run {
                println!(
                    "🔄 [DRY RUN] {:?} -> {:?}",
                    op.from.file_name().unwrap_or_default(),
                    op.to.file_name().unwrap_or_default()
                );
                success_count += 1;
            } else {
                match fs::rename(&op.from, &op.to) {
                    Ok(()) => {
                        if !self.args.quiet {
                            println!(
                                "✅ Renombrado: {:?} -> {:?}",
                                op.from.file_name().unwrap_or_default(),
                                op.to.file_name().unwrap_or_default()
                            );
                        }
                        success_count += 1;
                    }
                    Err(e) => {
                        eprintln!(
                            "❌ Error renombrando {:?}: {}",
                            op.from.file_name().unwrap_or_default(),
                            e
                        );
                        error_count += 1;
                    }
                }
            }
        }

        if !self.args.quiet {
            println!("\n📈 Resumen:");
            println!("  ✅ Éxitos: {}", success_count);
            if error_count > 0 {
                println!("  ❌ Errores: {}", error_count);
            }
            if self.args.dry_run {
                println!("  ℹ️ Modo de prueba activado - no se renombraron archivos realmente");
            }
        }

        Ok(())
    }

    fn run(&self) -> Result<()> {
        let (subtitles, videos) = self.categorize_files()?;
        let operations = self.plan_renames(subtitles, videos);
        self.execute_renames(operations)?;
        Ok(())
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Mostrar ayuda si no se proporcionan regex
    if args.srt_regex.is_none() && args.mkv_regex.is_none() {
        eprintln!("❌ Debes proporcionar al menos un regex.");
        eprintln!("\n📚 Ejemplos de uso:");
        eprintln!("  # Básico con regex para ambos tipos de archivo:");
        eprintln!("  sub-renamer --srt-regex 'S(\\d{{2}})E(\\d{{2}})' --mkv-regex 'S(\\d{{2}})E(\\d{{2}})'");
        eprintln!("\n  # Con extensiones múltiples y modo recursivo:");
        eprintln!("  sub-renamer --srt-regex 'S(\\d{{2}})E(\\d{{2}})' --srt-ext srt,ass,vtt --video-ext mkv,mp4,avi --recursive");
        eprintln!("\n  # Modo de prueba (no renombra realmente):");
        eprintln!("  sub-renamer --srt-regex 'S(\\d{{2}})E(\\d{{2}})' --dry-run");
        eprintln!("\n  # En directorio específico:");
        eprintln!("  sub-renamer --srt-regex 'S(\\d{{2}})E(\\d{{2}})' --directory /path/to/episodes");
        
        std::process::exit(1);
    }

    let renamer = SubtitleRenamer::new(args)?;
    renamer.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_extensions() {
        assert_eq!(
            SubtitleRenamer::parse_extensions("srt,ass,vtt"),
            vec!["srt", "ass", "vtt"]
        );
        assert_eq!(
            SubtitleRenamer::parse_extensions("mkv, mp4 , avi"),
            vec!["mkv", "mp4", "avi"]
        );
        assert_eq!(
            SubtitleRenamer::parse_extensions(""),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_extract_episode_id() -> Result<()> {
        let temp_dir = TempDir::new()?;
        
        // Test con regex que captura solo temporada
        let args1 = Args {
            srt_regex: Some(r"S(\d{2})E\d{2}".to_string()),
            mkv_regex: Some(r"S(\d{2})E\d{2}".to_string()),
            srt_ext: "srt".to_string(),
            video_ext: "mkv".to_string(),
            directory: temp_dir.path().to_path_buf(),
            recursive: false,
            dry_run: false,
            quiet: false,
            verbose: false,
        };

        let renamer1 = SubtitleRenamer::new(args1)?;
        let test_path1 = temp_dir.path().join("Show.S01E05.1080p.mkv");
        fs::write(&test_path1, b"")?;

        let episode_id1 = renamer1.extract_episode_id(&test_path1, false);
        assert_eq!(episode_id1, Some("01".to_string()));

        // Test con regex que captura temporada y episodio
        let args2 = Args {
            srt_regex: Some(r"(S\d{2}E\d{2})".to_string()),
            mkv_regex: Some(r"(S\d{2}E\d{2})".to_string()),
            srt_ext: "srt".to_string(),
            video_ext: "mkv".to_string(),
            directory: temp_dir.path().to_path_buf(),
            recursive: false,
            dry_run: false,
            quiet: false,
            verbose: false,
        };

        let renamer2 = SubtitleRenamer::new(args2)?;
        let test_path2 = temp_dir.path().join("Show.S01E05.1080p.mkv");
        fs::write(&test_path2, b"")?;

        let episode_id2 = renamer2.extract_episode_id(&test_path2, false);
        assert_eq!(episode_id2, Some("S01E05".to_string()));

        // Test con regex que captura múltiples grupos
        let args3 = Args {
            srt_regex: Some(r"S(\d{2})E(\d{2})".to_string()),
            mkv_regex: Some(r"S(\d{2})E(\d{2})".to_string()),
            srt_ext: "srt".to_string(),
            video_ext: "mkv".to_string(),
            directory: temp_dir.path().to_path_buf(),
            recursive: false,
            dry_run: false,
            quiet: false,
            verbose: false,
        };

        let renamer3 = SubtitleRenamer::new(args3)?;
        let test_path3 = temp_dir.path().join("Show.S01E05.1080p.mkv");
        fs::write(&test_path3, b"")?;

        let episode_id3 = renamer3.extract_episode_id(&test_path3, false);
        // Con múltiples grupos, solo toma el primer grupo de captura
        assert_eq!(episode_id3, Some("01".to_string()));

        Ok(())
    }

    #[test]
    fn test_extract_episode_id_various_formats() -> Result<()> {
        let temp_dir = TempDir::new()?;
        
        // Test con diferentes formatos de nombres de archivo
        let test_cases = vec![
            // (regex, filename, expected_result)
            (r"(S\d{2}E\d{2})", "Show.S01E05.1080p.mkv", Some("S01E05")),
            (r"(\d{1,2}x\d{2})", "Show.1x05.1080p.mkv", Some("1x05")),
            (r"Episode\.(\d+)", "Show.Episode.5.mkv", Some("5")),
            (r"Ep(\d+)", "Show.Ep05.mkv", Some("05")),
            (r"(S\d{2}E\d{2})", "Show.no.match.mkv", None),
        ];

        for (regex_str, filename, expected) in test_cases {
            let args = Args {
                srt_regex: Some(regex_str.to_string()),
                mkv_regex: Some(regex_str.to_string()),
                srt_ext: "srt".to_string(),
                video_ext: "mkv".to_string(),
                directory: temp_dir.path().to_path_buf(),
                recursive: false,
                dry_run: false,
                quiet: false,
                verbose: false,
            };

            let renamer = SubtitleRenamer::new(args)?;
            let test_path = temp_dir.path().join(filename);
            fs::write(&test_path, b"")?;

            let episode_id = renamer.extract_episode_id(&test_path, false);
            assert_eq!(
                episode_id,
                expected.map(|s| s.to_string()),
                "Failed for regex '{}' and filename '{}'",
                regex_str,
                filename
            );
        }

        Ok(())
    }
}