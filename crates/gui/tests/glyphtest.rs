#[test]
fn glyphs() {
    let ctx = egui::Context::default();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| { ui.label("x"); });
    });
    let fid = egui::FontId::proportional(12.0);
    for s in ["⁚","︙","｜","∶","¦","‖","║","|","⫶","⁝","⋮","፡","።","˸","ː","∴","⁘","⁙","⋰","⋱","❘","❙","❚","▎","▏","▕","│","┃","︰","ᛝ","።"] {
        if ctx.fonts(|f| f.has_glyphs(&fid, s)) { print!("{s} "); }
    }
    println!();
}
