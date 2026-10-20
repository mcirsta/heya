//! Synthetic typography fixture. Run through scripts/ui-offscreen.sh.
use slint::ComponentHandle;
slint::slint! {
component Sample inherits Rectangle {
    in property <string> family;
    in property <bool> dark;
    background: root.dark ? #111b21 : white;
    Text {x: 20px; y: 8px; text: root.family; font-family: "Inter"; font-size: 13px; color: #515f67;}
    Text {x: 20px; y: 40px; text: "A little room for better conversations. Salut, bună dimineața!"; font-family: root.family; font-size: 14.5px; font-weight:400; color: root.dark ? #e9edef : #111b21;}
    Text {x: 20px; y: 78px; text: "A little room for better conversations. Salut, bună dimineața!"; font-family: root.family; font-size: 21.75px; font-weight:400; color: root.dark ? #e9edef : #111b21;}
    Text {x: 20px; y: 124px; text: "Ada Lovelace — regular"; font-family: root.family; font-size: 14.5px; font-weight:400; color: root.dark ? #e9edef : #111b21;}
    Text {x: 280px; y: 124px; text: "Ada Lovelace — medium"; font-family: root.family; font-size: 14.5px; font-weight:500; color: root.dark ? #e9edef : #111b21;}
}
export component FontProbe inherits Window {
    in property <bool> dark;
    title: "Heya font probe"; width: 840px; height: 342px; background: #e9edef;
    Sample {x:0px; y:0px; width:parent.width; height:170px; family:"Inter"; dark:root.dark;}
    Sample {x:0px; y:172px; width:parent.width; height:170px; family:"Adwaita Sans"; dark:root.dark;}
}

}
fn main() -> Result<(), slint::PlatformError> {
    let ui = FontProbe::new()?;
    ui.set_dark(std::env::args().any(|arg| arg == "--dark"));
    ui.run()
}
