import io

# 1) ConversionPlan gains an explicit player selector.
p = 'core/src/convert/mod.rs'
s = io.open(p, encoding='utf-8').read()
old = '''    /// Stop after this many chunks (testing aid; `None` converts everything).
    pub chunk_limit: Option<usize>,
}'''
new = '''    /// Stop after this many chunks (testing aid; `None` converts everything).
    pub chunk_limit: Option<usize>,
    /// Which Java player becomes Bedrock's local player.
    ///
    /// Matched against the player file stem (the UUID), case-insensitively, as
    /// a full value or a prefix. `None` picks the most recently played player -
    /// never an arbitrary one - and reports the choice.
    pub player: Option<String>,
}'''
assert old in s, 'plan field'
s = s.replace(old, new, 1)

old = '''            chunk_limit: None,
        }'''
new = '''            chunk_limit: None,
            player: None,
        }'''
assert old in s, 'plan default'
s = s.replace(old, new, 1)
io.open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('plan.player added')

# 2) WorldPort picks the player deterministically and reports it.
p = 'core/src/convert/be_to_je.rs'
s = io.open(p, encoding='utf-8').read()
old = '''    // Modern worlds keep `playerdata/<uuid>.dat`; classic ones embed `Data.Player`.
    let player = if let Some((_, path)) = world.player_files()?.into_iter().next() {
        je_nbt::read_gzip(&path).ok()
    } else {'''
new = '''    // Modern worlds keep `playerdata/<uuid>.dat`; classic ones embed `Data.Player`.
    // Picking "the first file" would be arbitrary: a world played by several
    // accounts has several, and which one became the Bedrock player would
    // depend on sort order. A selector wins when given; otherwise the most
    // recently played player is used, and the choice is always reported.
    let chosen = choose_player(world, plan)?;
    let player = if let Some((stem, path)) = chosen {
        report.info("player", format!("converting player {stem} as the local player"));
        je_nbt::read_gzip(&path).ok()
    } else {'''
assert old in s, 'player read'
s = s.replace(old, new, 1)
io.open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('player selection wired')
