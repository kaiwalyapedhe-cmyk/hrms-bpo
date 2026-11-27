//! Payslip PDF generation (printpdf). The layout is deliberately plain so the
//! document is a faithful, printable statutory record rather than a brochure.

use printpdf::*;
use rust_decimal::Decimal;
use std::io::{BufWriter, Cursor};

pub struct PdfLine {
    pub label: String,
    pub amount: Decimal,
    pub taxable: bool,
}

pub struct PayslipPdfData {
    pub org_name: String,
    pub org_tan: String,
    pub pf_code: String,
    pub esi_code: String,
    pub employee_name: String,
    pub employee_no: String,
    pub pan_last4: String,
    pub uan: String,
    pub designation: String,
    pub department: String,
    pub work_state: String,
    pub period: String,
    pub paid_days: String,
    pub lop_days: String,
    pub earnings: Vec<PdfLine>,
    pub deductions: Vec<PdfLine>,
    pub employer_contributions: Vec<PdfLine>,
    pub gross: Decimal,
    pub total_deductions: Decimal,
    pub net_pay: Decimal,
    pub employer_cost: Decimal,
    pub pf_wages: Decimal,
    pub esi_wages: Decimal,
    pub pt: Decimal,
    pub tds: Decimal,
    pub taxable_income: Decimal,
    pub tax_regime: String,
    pub messages: Vec<String>,
}

/// Indian digit grouping: 12,34,567.89
fn money(value: Decimal) -> String {
    let value = value.round_dp(2);
    let negative = value < Decimal::ZERO;
    let abs = value.abs();
    let whole = abs.trunc();
    let paise = ((abs - whole) * Decimal::from(100)).round();
    let digits = whole.to_string();

    let (head, tail) = digits.split_at(digits.len().saturating_sub(3));
    let mut parts: Vec<String> = Vec::new();
    if !head.is_empty() {
        let mut rest = head.to_string();
        while rest.len() > 2 {
            let split = rest.len() - 2;
            parts.push(rest[split..].to_string());
            rest.truncate(split);
        }
        if !rest.is_empty() {
            parts.push(rest);
        }
        parts.reverse();
    }
    parts.push(tail.to_string());
    format!(
        "{}{}.{:02}",
        if negative { "-" } else { "" },
        parts.join(","),
        paise
    )
}

fn text(layer: &PdfLayerReference, font: &IndirectFontRef, size: f32, x: f32, y: f32, value: &str) {
    layer.use_text(value, size, Mm(x), Mm(y), font);
}

/// Renders the payslip and returns the PDF bytes.
pub fn payslip_pdf(data: &PayslipPdfData) -> Result<Vec<u8>, String> {
    let (doc, page, layer_idx) = PdfDocument::new(
        format!("Payslip {}", data.period),
        Mm(210.0),
        Mm(297.0),
        "Payslip",
    );
    let font = doc
        .add_builtin_font(BuiltinFont::Helvetica)
        .map_err(|e| format!("pdf font: {e}"))?;
    let bold = doc
        .add_builtin_font(BuiltinFont::HelveticaBold)
        .map_err(|e| format!("pdf font: {e}"))?;
    let layer = doc.get_page(page).get_layer(layer_idx);

    let mut y = 282.0f32;

    text(&layer, &bold, 15.0, 15.0, y, &data.org_name);
    y -= 6.0;
    text(&layer, &font, 9.0, 15.0, y, "Payslip — confidential");
    text(&layer, &font, 9.0, 150.0, y, &format!("Period: {}", data.period));
    y -= 5.0;
    text(
        &layer,
        &font,
        8.0,
        15.0,
        y,
        &format!(
            "TAN: {}   PF: {}   ESI: {}",
            data.org_tan, data.pf_code, data.esi_code
        ),
    );

    y -= 4.0;
    layer.set_outline_thickness(0.3);
    layer.add_line(Line {
        points: vec![
            (Point::new(Mm(15.0), Mm(y)), false),
            (Point::new(Mm(195.0), Mm(y)), false),
        ],
        is_closed: false,
    });

    y -= 7.0;
    text(&layer, &bold, 10.0, 15.0, y, "Employee details");
    y -= 5.0;
    let left = [
        ("Name", data.employee_name.clone()),
        ("Employee No", data.employee_no.clone()),
        ("Designation", data.designation.clone()),
        ("Department", data.department.clone()),
    ];
    let right = [
        ("PAN", format!("xxxxx{}", data.pan_last4)),
        ("UAN", data.uan.clone()),
        ("Work state", data.work_state.clone()),
        (
            "Days paid / LOP",
            format!("{} / {}", data.paid_days, data.lop_days),
        ),
    ];
    let mut row_y = y;
    for (label, value) in &left {
        text(&layer, &font, 8.1, 15.0, row_y, &format!("{label}: {value}"));
        row_y -= 4.6;
    }
    let mut row_y = y;
    for (label, value) in &right {
        text(&layer, &font, 8.1, 110.0, row_y, &format!("{label}: {value}"));
        row_y -= 4.6;
    }
    y = row_y - 3.0;

    // Earnings / deductions side by side.
    y -= 4.0;
    text(&layer, &bold, 10.0, 15.0, y, "Earnings");
    text(&layer, &bold, 10.0, 110.0, y, "Deductions");
    y -= 5.0;
    let earnings_start = y;
    for line in &data.earnings {
        text(&layer, &font, 8.1, 15.0, y, &line.label);
        text(&layer, &font, 8.1, 75.0, y, &money(line.amount));
        y -= 4.6;
    }
    text(&layer, &bold, 8.6, 15.0, y, "Gross earnings");
    text(&layer, &bold, 8.6, 75.0, y, &money(data.gross));

    let mut dy = earnings_start;
    for line in &data.deductions {
        text(&layer, &font, 8.1, 110.0, dy, &line.label);
        text(&layer, &font, 8.1, 172.0, dy, &money(line.amount));
        dy -= 4.6;
    }
    text(&layer, &bold, 8.6, 110.0, dy, "Total deductions");
    text(&layer, &bold, 8.6, 172.0, dy, &money(data.total_deductions));

    y = y.min(dy) - 8.0;
    text(&layer, &bold, 11.0, 15.0, y, "Net pay");
    text(&layer, &bold, 11.0, 172.0, y, &money(data.net_pay));
    y -= 6.0;
    text(&layer, &font, 8.0, 15.0, y, "Amounts in INR. Net pay = gross earnings - total deductions.");

    y -= 8.0;
    text(&layer, &bold, 10.0, 15.0, y, "Employer contributions (not part of net pay)");
    y -= 5.0;
    for line in &data.employer_contributions {
        text(&layer, &font, 8.1, 15.0, y, &line.label);
        text(&layer, &font, 8.1, 75.0, y, &money(line.amount));
        y -= 4.6;
    }
    text(&layer, &bold, 8.6, 15.0, y, "Total cost to company");
    text(&layer, &bold, 8.6, 75.0, y, &money(data.employer_cost));

    y -= 8.0;
    text(&layer, &bold, 10.0, 15.0, y, "Statutory summary");
    y -= 5.0;
    let statutory = [
        ("PF wages", money(data.pf_wages)),
        ("ESI wages", money(data.esi_wages)),
        ("Professional tax", money(data.pt)),
        ("TDS deducted", money(data.tds)),
        ("Projected taxable income (annual)", money(data.taxable_income)),
        ("Tax regime", data.tax_regime.clone()),
    ];
    for (label, value) in &statutory {
        text(&layer, &font, 8.1, 15.0, y, &format!("{label}: {value}"));
        y -= 4.4;
    }

    y -= 5.0;
    text(&layer, &bold, 9.0, 15.0, y, "Payroll messages");
    y -= 4.6;
    if data.messages.is_empty() {
        text(&layer, &font, 7.8, 15.0, y, "None");
    } else {
        for message in &data.messages {
            let line = if message.len() > 118 {
                format!("{}…", &message[..117])
            } else {
                message.clone()
            };
            text(&layer, &font, 7.6, 15.0, y, &line);
            y -= 4.0;
            if y < 25.0 {
                break;
            }
        }
    }

    text(
        &layer,
        &font,
        7.0,
        15.0,
        18.0,
        "Computer-generated payslip. Retain for 7 years as required by the Income-tax Act and EPF & MP Act.",
    );

    let mut writer = BufWriter::new(Cursor::new(Vec::<u8>::new()));
    doc.save(&mut writer).map_err(|e| format!("pdf save: {e}"))?;
    let cursor = writer
        .into_inner()
        .map_err(|e| format!("pdf buffer: {e}"))?;
    Ok(cursor.into_inner())
}
