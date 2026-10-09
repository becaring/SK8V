//! Compact HUD presentation. The APT movie still owns content,
//! visibility and animation; this projection changes only draw positions.
use crate::{apt_movie::Movie, apt_scene::{self, Draw, Shapes}, apt_vm::{Value, Vm}};

/// Draws each visible root child on its own and moves it to the compact
/// position. Scene traversal reads properties only, so the children are
/// shown one at a time by setting their `_visible` on `vm` and every field
/// is put back before returning: no script runs and nothing else changes.
pub fn compact(movie:&Movie, vm:&mut Vm, shapes:&Shapes, edge:f32)->Result<Vec<Draw>,String> {
    let children:Vec<_>=movie.instances[&movie.root].children.values().copied().collect();
    let shown:Vec<bool>=children.iter().map(|&id|vm.get(id,"_visible").truth()).collect();
    let saved:Vec<Option<Value>>=children.iter().map(|&id|vm.objects.get(id).and_then(|o|o.fields.get("_visible").cloned())).collect();
    let result=project(movie,vm,shapes,edge,&children,&shown);
    for (&id,old) in children.iter().zip(saved) {
        match old {
            Some(v)=>{vm.set(id,"_visible",v)?;}
            None=>{if let Some(o)=vm.objects.get_mut(id) {o.fields.remove("_visible");}}
        }
    }
    result
}

fn project(movie:&Movie, vm:&mut Vm, shapes:&Shapes, edge:f32, children:&[usize], shown:&[bool])->Result<Vec<Draw>,String> {
    for &id in children {vm.set(id,"_visible",Value::Bool(false))?;}
    let mut result=Vec::new();
    for (&id,_) in children.iter().zip(shown).filter(|(_,s)|**s) {
        vm.set(id,"_visible",Value::Bool(true))?;
        let draws=apt_scene::draw(movie,vm,shapes);
        vm.set(id,"_visible",Value::Bool(false))?;
        let mut draws=draws?;
        if draws.is_empty() {continue;}
        let name=movie.instances[&id].placement.as_ref().map(|p|p.name.as_str()).unwrap_or("");
        // Anchor text using sharp glyphs; glow extents must not make it jump.
        let sharp_right=draws.iter().filter(|d|movie.text_assets.fonts.values()
            .any(|font|font.foreground.is_none()&&font.texture==d.texture))
            .flat_map(|d|&d.vertices).map(|v|v.position[0]).reduce(f32::max);
        let right=sharp_right.unwrap_or_else(||draws.iter().flat_map(|d|&d.vertices)
            .map(|v|v.position[0]).fold(f32::NEG_INFINITY,f32::max));
        let text_right=1063.04-edge;
        let (scale,y_offset)=match name {
            "mcScoreModes"=>(0.5332,298.45),
            "mcLinescoreHolder"=>(0.86,99.0),
            n if n.starts_with("bigTrick_mc")||n.starts_with("mcTrickAnim")=>(0.774,177.0),
            "multiplier_mc"|"mcSwitch"=>(0.86,91.93333),
            _=>return Err(format!("Unmapped HUD presentation group {name}")),
        };
        for d in &mut draws {for v in &mut d.vertices {
            v.position[0]=if matches!(name,"multiplier_mc"|"mcSwitch") {
                1124.96-edge+(v.position[0]-(164.0+edge))*scale
            } else {text_right+(v.position[0]-right)*scale};
            v.position[1]=v.position[1]*scale+y_offset;
        }}
        result.extend(draws);
    }
    Ok(result)
}
