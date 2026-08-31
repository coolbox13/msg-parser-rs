use printpdf::image_crate::io::Reader as ImageReader;
use msg_parser::{Attachment, Outlook, Person};
use printpdf::{
    BuiltinFont, Image, ImageTransform, Mm, PdfDocument, PdfDocumentReference, PdfLayerIndex,
    PdfPageIndex,
};
use std::fs::{self, File};
use std::io::{BufWriter, Cursor};
use std::path::{Path, PathBuf};

const PAGE_W_MM: f32 = 210.0;
const PAGE_H_MM: f32 = 297.0;
const MARGIN_MM: f32 = 15.0;
const LINE_HEIGHT_MM: f32 = 5.0;
const BODY_FONT_SIZE: f32 = 11.0;
const TITLE_FONT_SIZE: f32 = 14.0;
const CAPTION_FONT_SIZE: f32 = 10.0;

fn attachment_filename(attach: &Attachment) -> String {
    if !attach.long_file_name.is_empty() {
        attach.long_file_name.clone()
    } else if !attach.file_name.is_empty() {
        attach.file_name.clone()
    } else if !attach.display_name.is_empty() {
        attach.display_name.clone()
    } else {
        format!("attachment{}", attach.extension)
    }
}

fn format_people(people: &[Person]) -> String {
    people
        .iter()
        .map(|person| {
            if person.email.is_empty() {
                person.name.clone()
            } else if person.name.is_empty() {
                person.email.clone()
            } else {
                format!("{} <{}>", person.name, person.email)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let paragraph = paragraph.trim_end();
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut start = 0;
        while start < paragraph.len() {
            let end = (start + max_chars).min(paragraph.len());
            let mut split_at = end;
            if end < paragraph.len() {
                if let Some(space) = paragraph[start..end].rfind(' ') {
                    split_at = start + space;
                }
            }
            if split_at <= start {
                split_at = end;
            }
            lines.push(paragraph[start..split_at].trim_end().to_string());
            start = split_at;
            while start < paragraph.len() && paragraph.as_bytes()[start] == b' ' {
                start += 1;
            }
        }
    }
    lines
}

fn add_lines(
    doc: &PdfDocumentReference,
    mut page: PdfPageIndex,
    mut layer: PdfLayerIndex,
    mut y: f32,
    lines: &[String],
    font: &printpdf::IndirectFontRef,
    font_size: f32,
) -> (PdfPageIndex, PdfLayerIndex, f32) {
    for line in lines {
        if y < MARGIN_MM + LINE_HEIGHT_MM {
            let (new_page, new_layer) = doc.add_page(Mm(PAGE_W_MM), Mm(PAGE_H_MM), "Summary");
            page = new_page;
            layer = new_layer;
            y = PAGE_H_MM - MARGIN_MM;
        }
        doc.get_page(page)
            .get_layer(layer)
            .use_text(line, font_size, Mm(MARGIN_MM), Mm(y), font);
        y -= LINE_HEIGHT_MM;
    }
    (page, layer, y)
}

fn is_image_attachment(attach: &Attachment) -> bool {
    let name = attachment_filename(attach).to_ascii_lowercase();
    name.ends_with(".jpg")
        || name.ends_with(".jpeg")
        || name.ends_with(".png")
        || name.ends_with(".gif")
        || name.ends_with(".webp")
        || name.ends_with(".bmp")
        || attach.mime_tag.starts_with("image/")
}

fn render_combined_pdf(outlook: &Outlook, pdf_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let (doc, page, layer) = PdfDocument::new(
        if outlook.subject.is_empty() {
            "Email"
        } else {
            &outlook.subject
        },
        Mm(PAGE_W_MM),
        Mm(PAGE_H_MM),
        "Summary",
    );
    let font = doc.add_builtin_font(BuiltinFont::Helvetica)?;
    let font_bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;

    let mut summary_lines = vec![
        format!("Subject: {}", outlook.subject),
        format!(
            "From:    {} <{}>",
            outlook.sender.name, outlook.sender.email
        ),
    ];
    if !outlook.to.is_empty() {
        summary_lines.push(format!("To:      {}", format_people(&outlook.to)));
    }
    if !outlook.cc.is_empty() {
        summary_lines.push(format!("CC:      {}", format_people(&outlook.cc)));
    }
    if !outlook.message_delivery_time.is_empty() {
        summary_lines.push(format!("Date:    {}", outlook.message_delivery_time));
    } else if !outlook.client_submit_time.is_empty() {
        summary_lines.push(format!("Date:    {}", outlook.client_submit_time));
    }
    summary_lines.push(String::new());
    summary_lines.push("Body:".to_string());
    summary_lines.extend(wrap_text(&outlook.body, 90));

    let image_attachments: Vec<&Attachment> = outlook
        .attachments
        .iter()
        .filter(|attach| !attach.is_embedded_message() && !attach.payload_bytes.is_empty())
        .collect();

    if !image_attachments.is_empty() {
        summary_lines.push(String::new());
        summary_lines.push(format!("Attachments ({}):", image_attachments.len()));
        for attach in &image_attachments {
            summary_lines.push(format!("  - {}", attachment_filename(attach)));
        }
    }

    let mut y = PAGE_H_MM - MARGIN_MM;
    doc.get_page(page)
        .get_layer(layer)
        .use_text("Email", TITLE_FONT_SIZE, Mm(MARGIN_MM), Mm(y), &font_bold);
    y -= LINE_HEIGHT_MM * 1.5;

    let (_, _, _) = add_lines(&doc, page, layer, y, &summary_lines, &font, BODY_FONT_SIZE);

    for attach in image_attachments {
        if !is_image_attachment(attach) {
            continue;
        }

        let dynamic_image = ImageReader::new(Cursor::new(&attach.payload_bytes))
            .with_guessed_format()?
            .decode()?;
        let (page, layer) = doc.add_page(Mm(PAGE_W_MM), Mm(PAGE_H_MM), "Attachment");
        let current_layer = doc.get_page(page).get_layer(layer);

        let caption = attachment_filename(attach);
        current_layer.use_text(
            &caption,
            CAPTION_FONT_SIZE,
            Mm(MARGIN_MM),
            Mm(PAGE_H_MM - MARGIN_MM),
            &font_bold,
        );

        let img_w = dynamic_image.width() as f32;
        let img_h = dynamic_image.height() as f32;
        let dpi = 300.0;
        let w_mm = (img_w / dpi) * 25.4;
        let h_mm = (img_h / dpi) * 25.4;
        let max_w = PAGE_W_MM - 2.0 * MARGIN_MM;
        let max_h = PAGE_H_MM - 2.0 * MARGIN_MM - 12.0;
        let scale = (max_w / w_mm).min(max_h / h_mm);

        let scaled_w = w_mm * scale;
        let scaled_h = h_mm * scale;
        let x = MARGIN_MM + (max_w - scaled_w) / 2.0;
        let y = MARGIN_MM + (max_h - scaled_h) / 2.0;

        let image = Image::from_dynamic_image(&dynamic_image);
        image.add_to_layer(
            current_layer,
            ImageTransform {
                translate_x: Some(Mm(x)),
                translate_y: Some(Mm(y)),
                rotate: None,
                scale_x: Some(scale),
                scale_y: Some(scale),
                dpi: Some(dpi),
            },
        );

    }

    let file = File::create(pdf_path)?;
    doc.save(&mut BufWriter::new(file))?;
    Ok(())
}

fn convert_msg(msg_path: &Path, output_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let stem = msg_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("invalid msg filename")?;
    let out_dir = output_root.join(stem);
    fs::create_dir_all(&out_dir)?;

    let outlook = Outlook::from_path(msg_path)?;

    let pdf_path = output_root.join(format!("{stem}.pdf"));
    render_combined_pdf(&outlook, &pdf_path)?;

    for attach in &outlook.attachments {
        if attach.is_embedded_message() || attach.payload_bytes.is_empty() {
            continue;
        }
        let name = attachment_filename(attach);
        fs::write(out_dir.join(&name), &attach.payload_bytes)?;
    }

    println!(
        "Converted: {} -> {} + {}",
        msg_path.display(),
        pdf_path.display(),
        out_dir.display()
    );
    Ok(())
}

fn main() {
    let input_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "bonnen".into()),
    );
    let output_dir = PathBuf::from(
        std::env::args()
            .nth(2)
            .unwrap_or_else(|| "bonnen/converted".into()),
    );

    fs::create_dir_all(&output_dir).expect("failed to create output directory");

    let mut msg_files: Vec<PathBuf> = fs::read_dir(&input_dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", input_dir.display()))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "msg"))
        .collect();
    msg_files.sort();

    if msg_files.is_empty() {
        eprintln!("No .msg files found in {}", input_dir.display());
        std::process::exit(1);
    }

    let mut failed = false;
    for msg_path in msg_files {
        if let Err(err) = convert_msg(&msg_path, &output_dir) {
            eprintln!("Failed {}: {err}", msg_path.display());
            failed = true;
        }
    }

    if failed {
        std::process::exit(1);
    }
}
