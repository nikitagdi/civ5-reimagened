//! What music plays: the exe's `cvAudioSimMainGameMusic` (re-b15 ITEM 3) as a state machine over the game's state.
//!
//! The director decides which song plays and how it comes in (a new song, the old one held to resume later, a held song
//! resumed); the engine (`UiAudio::play_music`) plays it with the 2 s crossfade. Time comes in as a parameter, the
//! shuffles from a seeded generator, so a test sees the same choices as a game.
//!
//! The rules (exe addresses of the Linux build):
//! - `Update` 0x08b75070: the logic runs once a second, or at once when a new song is forced.
//! - `UpdateMusic` 0x08b750e2: nothing while the loading screen is up; when the song ended, 20 s of silence
//!   ([`SILENCE_GAP_MS`]) before the next one of the playlist.
//! - `UpdateWarStatus` 0x08b753ee: the war playlist while the active team is at war with anyone (in a leader screen:
//!   with that leader); a change forces a new song.
//! - `PickNonPlaylistTrack` 0x08b75560: the first song of a game is the active leader's own theme, the game over screen
//!   plays his WAR theme, a leader screen plays that leader's PEACE or WAR theme and holds the playlist song; leaving it
//!   plays his WAR theme once if at war with him, else resumes the held song where it stopped.
//! - `PlayNewSong` 0x08b75bf4: the shuffled playlist advances, a wrap reshuffles it (no repeat across the wrap).
//! - `UpdatePlayerTracks` 0x08b748e8: in the Renaissance a few songs of the civilizations met join the playlist.
//!
//! SIMPLIFIED(SIMP-exe-1197): the leader screen's looping ambience (`AS2D_AMBIENCE_<LEADER>_AMBIENCE`) and the leaders'
//! spoken lines (their trigger is in the leader scene's data, not found) are not played; the music does not stop while
//! the browser tab is hidden (the exe's application flag 0x1eac).

/// The logic runs this often (`Update`: tick 1.0 s).
pub const UPDATE_MS: u64 = 1000;
/// Silence after a playlist song ended (`UpdateMusic`: 0x41a00000 = 20.0 s).
pub const SILENCE_GAP_MS: u64 = 20_000;
/// Every song change crossfades this long (`PlayMusic` 0x08b75f64: 0x40000000 = 2.0 s, both ways).
pub const CROSSFADE_S: f32 = 2.0;
/// The main menu loop (`cvAudioSystem::GetMainMenuMusicNormalScriptID`; the `_51` variant is for surround output).
pub const MAIN_MENU_SCRIPT: &str = "AS2D_MUSIC_MAIN_MENU";
/// `AudioParams.xml <Music>`: songs of the civilizations met join the playlist from this era (`ERA_RENAISSANCE`), this
/// many percent per era, at most this many percent. SIMPLIFIED(SIMP-exe-1195): the three tags are mapped to the fields
/// `UpdatePlayerTracks` reads by their names (their loader was not traced).
pub const BORDER_TRACKS_START_ERA: &str = "ERA_RENAISSANCE";
pub const BORDER_TRACKS_PERCENT_PER_ERA: i32 = 10;
pub const BORDER_TRACKS_MAX_PERCENT: i32 = 50;
/// The leader of `NO_PLAYER` (`BuildNonPlaylistCivSpecificSongName`) and the civilization of `NO_PLAYER`
/// (`BuildPlaylistTrackName`).
const NO_PLAYER_LEADER: &str = "WASHINGTON";
pub const NO_PLAYER_TAG: &str = "ENGLAND";

/// How a song comes in (`PlayMusic` mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicMode {
    /// The new song fades in, the old one fades out and ends.
    New,
    /// The new song fades in, the old one fades out and is held (to be resumed).
    HoldOld,
    /// The held song comes back where it stopped (a new start when nothing is held).
    Resume,
}

/// What the engine does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MusicCmd {
    Play { script: String, mode: MusicMode },
}

/// A player as the music sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MusicCiv {
    pub id: i32,
    /// `Leaders.Type`, e.g. `LEADER_WASHINGTON`.
    pub leader: String,
    /// The soundtrack of the civilization: `Civilizations.SoundtrackTag`, else its `Type` without `CIVILIZATION_`
    /// (see [`soundtrack_tag`]).
    pub tag: String,
    /// The active team has met it.
    pub met: bool,
    /// The active team is at war with its team.
    pub at_war: bool,
    /// A major civilization that is alive (the borrowing of songs takes only those).
    pub major_alive: bool,
}

/// The game as the music sees it, once a second.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MusicView {
    pub active: i32,
    /// The active player is a human (only a human's playlist borrows songs).
    pub human: bool,
    /// The active player's era (its index) and the index of [`BORDER_TRACKS_START_ERA`].
    pub era: i32,
    pub start_era: i32,
    /// Every player the music may name, the active one included.
    pub civs: Vec<MusicCiv>,
}

impl MusicView {
    fn civ(&self, id: i32) -> Option<&MusicCiv> {
        self.civs.iter().find(|c| c.id == id)
    }
}

/// Where the player is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicScene {
    /// The lobby: the main menu loop.
    Menu,
    /// A game loads: what played goes on, the game's music waits for the loading screen to close.
    Loading,
    Game,
}

/// The game events the music listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicEvent {
    /// `AILeaderMessage` (event 118) of a player: his leader screen is up.
    LeaderView(i32),
    /// `LeaveLeaderView` (event 39).
    LeaveLeaderView,
    /// `GameOverScreenShow` (event 136).
    GameOver,
}

/// `Civilizations.SoundtrackTag` if set, else the `Type` without `CIVILIZATION_`, upper case.
pub fn soundtrack_tag(soundtrack: Option<&str>, civ_type: &str) -> String {
    match soundtrack.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_ascii_uppercase(),
        None => civ_type
            .strip_prefix("CIVILIZATION_")
            .unwrap_or(civ_type)
            .to_ascii_uppercase(),
    }
}

/// `SONG_<TAG>_<PEACE|WAR>_<n>` (`BuildPlaylistTrackName` 0x08b76a46).
pub fn song_name(tag: &str, war: bool, n: u32) -> String {
    format!(
        "SONG_{}_{}_{n}",
        tag.to_ascii_uppercase(),
        if war { "WAR" } else { "PEACE" }
    )
}

/// `AS2D_LEADER_MUSIC_<leader without LEADER_>_<PEACE|WAR>` (`BuildNonPlaylistCivSpecificSongName` 0x08b759ca).
pub fn leader_song(leader: &str, war: bool) -> String {
    let l = leader.trim();
    let l = l.strip_prefix("LEADER_").unwrap_or(l);
    let l = if l.is_empty() { NO_PLAYER_LEADER } else { l };
    format!(
        "AS2D_LEADER_MUSIC_{}_{}",
        l.to_ascii_uppercase(),
        if war { "WAR" } else { "PEACE" }
    )
}

/// The number of songs of a list (`GetSongTagCount`): probes n = 1, 11, 21, ... while the script exists, then walks back
/// from the first missing probe to the highest n that exists.
pub fn song_count(tag: &str, war: bool, has: &dyn Fn(&str) -> bool) -> u32 {
    if !has(&song_name(tag, war, 1)) {
        return 0;
    }
    let mut p = 1;
    while p < 10_000 && has(&song_name(tag, war, p + 10)) {
        p += 10;
    }
    (p + 1..p + 10)
        .rev()
        .find(|n| has(&song_name(tag, war, *n)))
        .unwrap_or(p)
}

/// The songs of a list. SIMPLIFIED(SIMP-exe-1190): a number below the count that has no script is left out (the exe
/// keeps it and plays nothing for it, then waits the 20 s gap).
fn songs(tag: &str, war: bool, has: &dyn Fn(&str) -> bool) -> Vec<String> {
    (1..=song_count(tag, war, has))
        .map(|n| song_name(tag, war, n))
        .filter(|s| has(s))
        .collect()
}

#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// A shuffled list of songs (`cvMusicPlaylist`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Playlist {
    tracks: Vec<String>,
    /// `None` before the first song (the exe's index -1: the first advance plays element 0).
    index: Option<usize>,
}

impl Playlist {
    pub fn tracks(&self) -> &[String] {
        &self.tracks
    }

    pub fn contains(&self, song: &str) -> bool {
        self.tracks.iter().any(|t| t == song)
    }

    pub fn current(&self) -> Option<&str> {
        self.index
            .and_then(|i| self.tracks.get(i))
            .map(String::as_str)
    }

    /// `RandomizeSongList`: swaps each element with a random one (`rand * count`), then swaps the first and the last
    /// song when the round would start with the song that ended the last one. SIMPLIFIED(SIMP-exe-1193): the worker's
    /// reading compares the new last song with the old last one; the rule kept is its stated effect (no song twice in a
    /// row across the wrap).
    fn shuffle(&mut self, rng: &mut Rng) {
        let old_last = self.tracks.last().cloned();
        let n = self.tracks.len();
        for i in 0..n {
            let j = rng.below(n);
            self.tracks.swap(i, j);
        }
        if n > 1 && self.tracks.first() == old_last.as_ref() {
            self.tracks.swap(0, n - 1);
        }
    }

    /// `AdvancePlaylistToNextTrack`: `true` when it wrapped to the start.
    fn advance(&mut self) -> bool {
        if self.tracks.is_empty() {
            return false;
        }
        let next = self.index.map_or(0, |i| (i + 1) % self.tracks.len());
        self.index = Some(next);
        next == 0 && self.tracks.len() > 1
    }
}

/// The state of the in-game music (the fields of `cvAudioSimMainGameMusic`).
#[derive(Debug, Clone)]
struct GameMusic {
    loading_up: bool,
    tick_left_ms: u64,
    gap_armed: bool,
    gap_left_ms: u64,
    first_song_pending: bool,
    force: bool,
    /// 0x42: the next song does not advance the playlist (leaving a leader screen resumes the held song).
    no_advance: bool,
    resume_saved: bool,
    saved: Option<String>,
    leader_view: Option<i32>,
    prev_leader_view: Option<i32>,
    game_over: bool,
    /// The leaders whose WAR theme was played on leaving their screen.
    war_theme_played: Vec<i32>,
    at_war: bool,
    prev_at_war: bool,
    peace: Playlist,
    war: Playlist,
    /// What the lists were built for: the era and the civilizations met (the exe rebuilds on EraChanged / TeamMet).
    built_for: Option<(i32, Vec<String>)>,
}

impl Default for GameMusic {
    fn default() -> Self {
        Self {
            loading_up: true,
            tick_left_ms: 0,
            gap_armed: false,
            gap_left_ms: 0,
            first_song_pending: true,
            force: false,
            no_advance: false,
            resume_saved: false,
            saved: None,
            leader_view: None,
            prev_leader_view: None,
            game_over: false,
            war_theme_played: Vec::new(),
            at_war: false,
            prev_at_war: false,
            peace: Playlist::default(),
            war: Playlist::default(),
            built_for: None,
        }
    }
}

/// Decides the music (see the module).
#[derive(Debug, Clone)]
pub struct MusicDirector {
    rng: Rng,
    scene: Option<MusicScene>,
    game: GameMusic,
    /// The song the director last asked for (`currentSong` 0x1c).
    current: Option<String>,
}

impl MusicDirector {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed | 1),
            scene: None,
            game: GameMusic::default(),
            current: None,
        }
    }

    /// The player whose leader screen the music follows now.
    pub fn leader_view(&self) -> Option<i32> {
        self.game.leader_view
    }

    /// The song asked for last.
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// The playlist that plays now (war or peace).
    pub fn playlist(&self) -> &Playlist {
        if self.game.at_war {
            &self.game.war
        } else {
            &self.game.peace
        }
    }

    pub fn peace_list(&self) -> &Playlist {
        &self.game.peace
    }

    pub fn war_list(&self) -> &Playlist {
        &self.game.war
    }

    /// One frame: `dt_ms` since the last call, where the player is, the game (once it is known), its events since the
    /// last call, whether the song asked for still sounds, and which scripts the sound table has.
    pub fn update(
        &mut self,
        dt_ms: u64,
        scene: MusicScene,
        view: Option<&MusicView>,
        events: &[MusicEvent],
        playing: bool,
        has: &dyn Fn(&str) -> bool,
    ) -> Vec<MusicCmd> {
        let mut out = Vec::new();
        let was = self.scene.replace(scene);
        match scene {
            MusicScene::Menu => {
                if was != Some(MusicScene::Menu) {
                    // The game is over for the music: the next one starts from the beginning.
                    self.game = GameMusic::default();
                }
                if self.current.as_deref() != Some(MAIN_MENU_SCRIPT) {
                    self.play(MAIN_MENU_SCRIPT.to_owned(), MusicMode::New, &mut out);
                }
                return out;
            }
            MusicScene::Loading => {
                if was == Some(MusicScene::Menu) {
                    self.game = GameMusic::default();
                }
                self.game.loading_up = true;
                // SIMPLIFIED(SIMP-exe-1191): the main menu loop goes on under the loading screen (the exe's
                // LOADING_SCREEN_MUSIC_OVERRIDE is defined nowhere; what the exe plays while loading is open).
                return out;
            }
            MusicScene::Game => {
                if was == Some(MusicScene::Menu) {
                    self.game = GameMusic::default();
                }
            }
        }
        let Some(view) = view else {
            // (the game is not known yet: as if the loading screen were still up)
            return out;
        };
        if self.game.loading_up {
            // `OnLoadingScreenClose` (event 170).
            self.game.loading_up = false;
            self.game.force = true;
            self.game.no_advance = false;
        }
        for e in events {
            match *e {
                // `OnLeaderViewMessage`: only a new leader changes the song.
                MusicEvent::LeaderView(p) => {
                    if self.game.leader_view != Some(p) {
                        self.game.leader_view = Some(p);
                        self.game.force = true;
                    }
                }
                // `OnLeaveLeaderView`: the held song comes back without the playlist moving on.
                MusicEvent::LeaveLeaderView => {
                    if self.game.leader_view.take().is_some() {
                        self.game.force = true;
                        self.game.no_advance = true;
                        self.game.resume_saved = true;
                    }
                }
                MusicEvent::GameOver => {
                    self.game.force = true;
                    self.game.game_over = true;
                }
            }
        }
        self.build_lists(view, has);
        // `Update`: the gap runs down every frame, the logic once a second or when forced.
        if self.game.gap_armed {
            self.game.gap_left_ms = self.game.gap_left_ms.saturating_sub(dt_ms);
        }
        self.game.tick_left_ms = self.game.tick_left_ms.saturating_sub(dt_ms);
        if self.game.force || self.game.tick_left_ms == 0 {
            self.update_music(view, playing, &mut out);
            self.game.tick_left_ms = UPDATE_MS;
        }
        out
    }

    fn play(&mut self, script: String, mode: MusicMode, out: &mut Vec<MusicCmd>) {
        self.current = Some(script.clone());
        out.push(MusicCmd::Play { script, mode });
    }

    /// `UpdateMusic`.
    fn update_music(&mut self, view: &MusicView, playing: bool, out: &mut Vec<MusicCmd>) {
        self.update_war_status(view);
        let g = &mut self.game;
        if !playing && !g.gap_armed {
            g.gap_armed = true;
            g.gap_left_ms = SILENCE_GAP_MS;
        }
        if g.force {
            g.gap_left_ms = 0;
        }
        if !playing || g.force {
            self.play_new_song(view, out);
        }
    }

    /// `UpdateWarStatus`.
    fn update_war_status(&mut self, view: &MusicView) {
        let g = &mut self.game;
        g.at_war = match g.leader_view {
            None => view.civs.iter().any(|c| c.id != view.active && c.at_war),
            Some(p) => view.civ(p).is_some_and(|c| c.at_war),
        };
        if g.at_war != g.prev_at_war {
            g.force = true;
        }
        g.prev_at_war = g.at_war;
    }

    fn leader_of(view: &MusicView, player: i32) -> &str {
        view.civ(player).map_or("", |c| c.leader.as_str())
    }

    /// `PickNonPlaylistTrack`: a leader's theme, or `None` for the playlist.
    fn pick_special(&mut self, view: &MusicView) -> Option<String> {
        let g = &mut self.game;
        if g.first_song_pending {
            g.first_song_pending = false;
            return Some(leader_song(Self::leader_of(view, view.active), g.at_war));
        }
        if g.game_over {
            g.game_over = false;
            return Some(leader_song(Self::leader_of(view, view.active), true));
        }
        if let Some(p) = g.leader_view {
            g.prev_leader_view = Some(p);
            return Some(leader_song(Self::leader_of(view, p), g.at_war));
        }
        if let Some(p) = g.prev_leader_view.take() {
            let at_war = view.civ(p).is_some_and(|c| c.at_war);
            if at_war && !g.war_theme_played.contains(&p) {
                g.war_theme_played.push(p);
                // The held song is given up: after the theme the playlist moves on.
                g.saved = None;
                g.no_advance = false;
                return Some(leader_song(Self::leader_of(view, p), true));
            }
        }
        // SIMPLIFIED(SIMP-exe-1192): the exe's flag 0x49 (after that WAR theme, the active leader's own WAR theme) is not
        // copied; its meaning is not verified (re-b15 item 3, Q5): after the WAR theme the playlist goes on.
        None
    }

    /// `PlayNewSong`.
    fn play_new_song(&mut self, view: &MusicView, out: &mut Vec<MusicCmd>) {
        let special = self.pick_special(view);
        let (song, mode) = match special {
            None => {
                if self.game.gap_left_ms > 0 {
                    return;
                }
                let mut mode = MusicMode::New;
                let mut song = None;
                if self.game.resume_saved
                    && let Some(s) = self.game.saved.take()
                {
                    song = Some(s);
                    mode = MusicMode::Resume;
                }
                let no_advance = std::mem::take(&mut self.game.no_advance);
                let at_war = self.game.at_war;
                let list = if at_war {
                    &mut self.game.war
                } else {
                    &mut self.game.peace
                };
                if !list.tracks.is_empty() && song.as_ref().is_none_or(|s| list.contains(s)) {
                    if (!no_advance || list.current().is_none()) && list.advance() {
                        list.shuffle(&mut self.rng);
                    }
                    song = list.current().map(str::to_owned);
                    self.game.gap_armed = false;
                }
                match song {
                    Some(s) => (s, mode),
                    None => {
                        self.game.force = false;
                        self.game.resume_saved = false;
                        return;
                    }
                }
            }
            Some(s) => {
                let mut mode = MusicMode::New;
                if self.current.as_deref() != Some(s.as_str())
                    && let Some(cur) = self.current.clone()
                    && self.playlist().contains(&cur)
                {
                    // The playlist song is held, to come back when the leader screen closes.
                    self.game.saved = Some(cur);
                    mode = MusicMode::HoldOld;
                }
                (s, mode)
            }
        };
        self.play(song, mode, out);
        self.game.force = false;
        self.game.resume_saved = false;
    }

    /// `UpdatePlayerTracks` for the active player, when its era or the civilizations met changed.
    fn build_lists(&mut self, view: &MusicView, has: &dyn Fn(&str) -> bool) {
        let met: Vec<String> = view
            .civs
            .iter()
            .filter(|c| c.id != view.active && c.met && c.major_alive)
            .map(|c| c.tag.clone())
            .collect();
        let key = (view.era, met.clone());
        if self.game.built_for.as_ref() == Some(&key) {
            return;
        }
        let first = self.game.built_for.is_none();
        self.game.built_for = Some(key);
        let own_tag = view
            .civ(view.active)
            .map_or_else(|| NO_PLAYER_TAG.to_owned(), |c| c.tag.clone());
        let pct = if view.human && view.era >= view.start_era {
            ((1 - view.era + view.start_era) * BORDER_TRACKS_PERCENT_PER_ERA)
                .min(BORDER_TRACKS_MAX_PERCENT)
        } else {
            0
        };
        // The exe rebuilds only when there is a share to take (later eras keep the list as it was).
        if !first && pct <= 0 {
            return;
        }
        for war in [false, true] {
            let mut list = songs(&own_tag, war, has);
            if pct > 0 && !met.is_empty() {
                borrow_songs(&mut list, &met, war, pct, has);
            }
            let mut p = Playlist {
                tracks: list,
                index: None,
            };
            p.shuffle(&mut self.rng);
            if war {
                self.game.war = p;
            } else {
                self.game.peace = p;
            }
        }
    }
}

/// `UpdatePlayerTracks`: `(own * 100 / pct) / 100 + 1` songs of the civilizations met join the list, spread evenly over
/// them, at most three passes.
fn borrow_songs(
    list: &mut Vec<String>,
    met: &[String],
    war: bool,
    pct: i32,
    has: &dyn Fn(&str) -> bool,
) {
    let own = list.len() as i32;
    let target = (own * 100 / pct) / 100 + 1;
    let per = (target / met.len() as i32).max(1);
    let others: Vec<Vec<String>> = met.iter().map(|t| songs(t, war, has)).collect();
    let mut taken = 0;
    for _pass in 0..3 {
        for o in &others {
            let mut k = 0;
            for s in o {
                if taken >= target || k >= per {
                    break;
                }
                if !list.contains(s) {
                    list.push(s.clone());
                    taken += 1;
                    k += 1;
                }
            }
        }
        if taken >= target {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_songs(name: &str) -> bool {
        // America: 19 peace, 12 war songs; Rome: 5 + 5; every leader has both themes.
        let count = |prefix: &str, n: u32| {
            name.strip_prefix(prefix)
                .and_then(|r| r.parse::<u32>().ok())
                .is_some_and(|i| (1..=n).contains(&i))
        };
        count("SONG_AMERICA_PEACE_", 19)
            || count("SONG_AMERICA_WAR_", 12)
            || count("SONG_ROME_PEACE_", 5)
            || count("SONG_ROME_WAR_", 5)
            || name.starts_with("AS2D_LEADER_MUSIC_")
            || name == MAIN_MENU_SCRIPT
    }

    fn civ(id: i32, leader: &str, tag: &str) -> MusicCiv {
        MusicCiv {
            id,
            leader: leader.into(),
            tag: tag.into(),
            met: id == 0,
            at_war: false,
            major_alive: true,
        }
    }

    fn view() -> MusicView {
        MusicView {
            active: 0,
            human: true,
            era: 0,
            start_era: 3,
            civs: vec![
                civ(0, "LEADER_WASHINGTON", "AMERICA"),
                civ(1, "LEADER_AUGUSTUS", "ROME"),
            ],
        }
    }

    /// Runs the director over `ms` in frames of 100 ms with what the engine says; returns the commands.
    fn run(
        d: &mut MusicDirector,
        ms: u64,
        v: &MusicView,
        events: &[MusicEvent],
        playing: bool,
    ) -> Vec<MusicCmd> {
        let mut out = d.update(100, MusicScene::Game, Some(v), events, playing, &has_songs);
        for _ in 1..ms / 100 {
            out.extend(d.update(100, MusicScene::Game, Some(v), &[], playing, &has_songs));
        }
        out
    }

    fn play(script: &str, mode: MusicMode) -> MusicCmd {
        MusicCmd::Play {
            script: script.into(),
            mode,
        }
    }

    fn in_game() -> MusicDirector {
        let mut d = MusicDirector::new(42);
        d.update(16, MusicScene::Menu, None, &[], false, &has_songs);
        d.update(16, MusicScene::Loading, None, &[], true, &has_songs);
        d
    }

    #[test]
    fn names_follow_the_exe_formats() {
        assert_eq!(song_name("america", false, 3), "SONG_AMERICA_PEACE_3");
        assert_eq!(song_name("AZTEC", true, 12), "SONG_AZTEC_WAR_12");
        assert_eq!(
            leader_song("LEADER_AUGUSTUS", true),
            "AS2D_LEADER_MUSIC_AUGUSTUS_WAR"
        );
        assert_eq!(leader_song("", false), "AS2D_LEADER_MUSIC_WASHINGTON_PEACE");
        assert_eq!(
            soundtrack_tag(Some("Aztec"), "CIVILIZATION_POLYNESIA"),
            "AZTEC"
        );
        assert_eq!(soundtrack_tag(Some(" "), "CIVILIZATION_AMERICA"), "AMERICA");
        assert_eq!(soundtrack_tag(None, "CIVILIZATION_ROME"), "ROME");
    }

    #[test]
    fn the_song_count_probes_in_tens_and_walks_back() {
        assert_eq!(song_count("AMERICA", false, &has_songs), 19);
        assert_eq!(song_count("AMERICA", true, &has_songs), 12);
        assert_eq!(song_count("ROME", false, &has_songs), 5);
        assert_eq!(song_count("NOPE", false, &has_songs), 0);
        // A hole between two probes ends the walk at the highest song that exists.
        let holey =
            |n: &str| n == "SONG_X_PEACE_1" || n == "SONG_X_PEACE_11" || n == "SONG_X_PEACE_14";
        assert_eq!(song_count("X", false, &holey), 14);
    }

    #[test]
    fn the_main_menu_plays_its_loop_once_and_the_loading_screen_keeps_it() {
        let mut d = MusicDirector::new(1);
        let c = d.update(16, MusicScene::Menu, None, &[], false, &has_songs);
        assert_eq!(c, [play(MAIN_MENU_SCRIPT, MusicMode::New)]);
        assert!(
            d.update(16, MusicScene::Menu, None, &[], true, &has_songs)
                .is_empty()
        );
        assert!(
            d.update(16, MusicScene::Loading, None, &[], true, &has_songs)
                .is_empty()
        );
        // The game is not known yet: nothing changes.
        assert!(
            d.update(16, MusicScene::Game, None, &[], true, &has_songs)
                .is_empty()
        );
    }

    #[test]
    fn the_first_song_of_a_game_is_the_active_leaders_peace_theme() {
        let mut d = in_game();
        let c = run(&mut d, 100, &view(), &[], true);
        assert_eq!(
            c,
            [play("AS2D_LEADER_MUSIC_WASHINGTON_PEACE", MusicMode::New)]
        );
        // While it plays nothing else comes.
        assert!(run(&mut d, 30_000, &view(), &[], true).is_empty());
    }

    #[test]
    fn at_war_the_first_song_is_the_war_theme_and_the_war_playlist_follows() {
        let mut d = in_game();
        let mut v = view();
        v.civs[1].at_war = true;
        let c = run(&mut d, 100, &v, &[], true);
        assert_eq!(
            c,
            [play("AS2D_LEADER_MUSIC_WASHINGTON_WAR", MusicMode::New)]
        );
        // The theme ends: 20 s of silence, then a war song.
        let c = run(&mut d, 30_000, &v, &[], false);
        assert_eq!(c.len(), 1);
        let MusicCmd::Play { script, mode } = &c[0];
        assert!(script.starts_with("SONG_AMERICA_WAR_"), "{script}");
        assert_eq!(*mode, MusicMode::New);
    }

    #[test]
    fn a_song_that_ended_is_followed_after_twenty_seconds_of_silence() {
        let mut d = in_game();
        run(&mut d, 100, &view(), &[], true);
        // The theme ended at t = 0 (the engine says so from now on).
        let c = run(&mut d, 19_000, &view(), &[], false);
        assert!(c.is_empty(), "silence for 20 s: {c:?}");
        let c = run(&mut d, 2_000, &view(), &[], false);
        assert_eq!(c.len(), 1, "{c:?}");
        let MusicCmd::Play { script, .. } = &c[0];
        assert!(script.starts_with("SONG_AMERICA_PEACE_"), "{script}");
    }

    #[test]
    fn the_playlist_plays_every_song_once_per_round_and_never_twice_in_a_row() {
        let mut d = in_game();
        run(&mut d, 100, &view(), &[], true);
        let mut heard = Vec::new();
        let mut t = 0;
        while heard.len() < 19 * 3 && t < 2_000_000 {
            // (every song ends at once: the next one comes after the gap)
            for c in run(&mut d, 1000, &view(), &[], false) {
                let MusicCmd::Play { script, .. } = c;
                heard.push(script);
            }
            t += 1000;
        }
        assert_eq!(heard.len(), 19 * 3);
        assert!(
            t >= 19 * 3 * 20_000,
            "a gap of 20 s before each song: {t} ms"
        );
        for round in heard.chunks(19) {
            let mut r = round.to_vec();
            r.sort();
            r.dedup();
            assert_eq!(r.len(), 19, "each song once in a round");
        }
        assert!(
            heard.windows(2).all(|w| w[0] != w[1]),
            "no song twice in a row"
        );
        assert!(heard.iter().all(|s| s.starts_with("SONG_AMERICA_PEACE_")));
    }

    #[test]
    fn a_war_switches_the_playlist_at_once_and_peace_switches_it_back() {
        let mut d = in_game();
        run(&mut d, 100, &view(), &[], true);
        run(&mut d, 25_000, &view(), &[], false);
        let mut v = view();
        v.civs[1].at_war = true;
        // A song plays; the war forces a new one at the next second's check (2 s crossfade), not after the song.
        let c = run(&mut d, 1000, &v, &[], true);
        assert_eq!(c.len(), 1, "{c:?}");
        let MusicCmd::Play { script, mode } = &c[0];
        assert!(script.starts_with("SONG_AMERICA_WAR_"), "{script}");
        assert_eq!(*mode, MusicMode::New);
        assert!(std::ptr::eq(d.playlist(), d.war_list()));
        let c = run(&mut d, 1000, &view(), &[], true);
        let MusicCmd::Play { script, .. } = &c[0];
        assert!(script.starts_with("SONG_AMERICA_PEACE_"), "{script}");
    }

    #[test]
    fn a_leader_screen_plays_his_theme_and_leaving_resumes_the_held_song() {
        let mut d = in_game();
        run(&mut d, 100, &view(), &[], true);
        run(&mut d, 25_000, &view(), &[], false);
        let song = d.current().unwrap().to_owned();
        assert!(song.starts_with("SONG_AMERICA_PEACE_"));
        let c = run(&mut d, 100, &view(), &[MusicEvent::LeaderView(1)], true);
        assert_eq!(
            c,
            [play("AS2D_LEADER_MUSIC_AUGUSTUS_PEACE", MusicMode::HoldOld)]
        );
        // The same leader speaking again changes nothing.
        assert!(run(&mut d, 3000, &view(), &[MusicEvent::LeaderView(1)], true).is_empty());
        let c = run(&mut d, 100, &view(), &[MusicEvent::LeaveLeaderView], true);
        assert_eq!(c, [play(&song, MusicMode::Resume)]);
    }

    #[test]
    fn leaving_a_leader_at_war_plays_his_war_theme_once_then_the_playlist() {
        let mut d = in_game();
        let mut v = view();
        v.civs[1].at_war = true;
        run(&mut d, 100, &v, &[], true);
        run(&mut d, 25_000, &v, &[], false);
        let c = run(&mut d, 100, &v, &[MusicEvent::LeaderView(1)], true);
        assert_eq!(
            c[0],
            play("AS2D_LEADER_MUSIC_AUGUSTUS_WAR", MusicMode::HoldOld)
        );
        let c = run(&mut d, 100, &v, &[MusicEvent::LeaveLeaderView], true);
        assert_eq!(c, [play("AS2D_LEADER_MUSIC_AUGUSTUS_WAR", MusicMode::New)]);
        // The second visit resumes the playlist instead.
        run(&mut d, 25_000, &v, &[], false);
        let held = d.current().unwrap().to_owned();
        run(&mut d, 100, &v, &[MusicEvent::LeaderView(1)], true);
        let c = run(&mut d, 100, &v, &[MusicEvent::LeaveLeaderView], true);
        assert_eq!(c, [play(&held, MusicMode::Resume)]);
    }

    #[test]
    fn the_game_over_screen_plays_the_active_leaders_war_theme() {
        let mut d = in_game();
        run(&mut d, 100, &view(), &[], true);
        let c = run(&mut d, 100, &view(), &[MusicEvent::GameOver], true);
        assert_eq!(
            c,
            [play("AS2D_LEADER_MUSIC_WASHINGTON_WAR", MusicMode::New)]
        );
    }

    #[test]
    fn leaving_the_game_goes_back_to_the_main_menu_and_the_next_game_starts_over() {
        let mut d = in_game();
        run(&mut d, 100, &view(), &[], true);
        let c = d.update(16, MusicScene::Menu, None, &[], true, &has_songs);
        assert_eq!(c, [play(MAIN_MENU_SCRIPT, MusicMode::New)]);
        d.update(16, MusicScene::Loading, None, &[], true, &has_songs);
        let c = run(&mut d, 100, &view(), &[], true);
        assert_eq!(
            c,
            [play("AS2D_LEADER_MUSIC_WASHINGTON_PEACE", MusicMode::New)]
        );
    }

    #[test]
    fn in_the_renaissance_a_few_songs_of_the_civilizations_met_join_the_playlist() {
        let mut d = in_game();
        let mut v = view();
        v.civs[1].met = true;
        run(&mut d, 100, &v, &[], true);
        assert_eq!(
            d.peace_list().tracks().len(),
            19,
            "the ancient era borrows nothing"
        );
        v.era = 3;
        run(&mut d, 100, &v, &[], true);
        // 10 %: 19 * 100 / 10 / 100 + 1 = 2 songs of Rome.
        let rome = |p: &Playlist| {
            p.tracks()
                .iter()
                .filter(|t| t.starts_with("SONG_ROME_"))
                .count()
        };
        assert_eq!(d.peace_list().tracks().len(), 21);
        assert_eq!(rome(d.peace_list()), 2);
        assert_eq!(rome(d.war_list()), 2, "12 * 100 / 10 / 100 + 1");
        // The next era has no share: the list stays as it was.
        v.era = 4;
        run(&mut d, 100, &v, &[], true);
        assert_eq!(rome(d.peace_list()), 2);
        // An AI's playlist borrows nothing.
        let mut ai = in_game();
        let mut v = view();
        v.civs[1].met = true;
        v.era = 3;
        v.human = false;
        run(&mut ai, 100, &v, &[], true);
        assert_eq!(rome(ai.peace_list()), 0);
    }

    #[test]
    fn a_reshuffle_does_not_put_the_last_song_first_again() {
        let mut rng = Rng(7);
        for _ in 0..200 {
            let mut p = Playlist {
                tracks: (1..=4).map(|n| format!("S{n}")).collect(),
                index: None,
            };
            let last = p.tracks.last().cloned();
            p.shuffle(&mut rng);
            assert_ne!(p.tracks.first(), last.as_ref());
        }
    }
}
