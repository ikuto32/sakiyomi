//! UI language for backend messages (status lines, results, errors).
//! The Japanese text is the key; other languages are looked up from it, and unknown
//! keys fall back to Japanese. Kifu file contents (KIF/KI2/CSA) are never translated.

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ja,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(0);

/// Accepts BCP-47-ish codes ("ja", "en", "en-US"); anything else is Japanese.
pub fn set_lang(code: &str) {
    let l = if code.to_ascii_lowercase().starts_with("en") { 1 } else { 0 };
    LANG.store(l, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == 1 { Lang::En } else { Lang::Ja }
}

pub fn tr(ja: &'static str) -> &'static str {
    match lang() {
        Lang::Ja => ja,
        Lang::En => EN.iter().find(|(k, _)| *k == ja).map(|(_, v)| *v).unwrap_or(ja),
    }
}

/// Translate a template and fill `{name}` placeholders.
pub fn trf(ja: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let mut s = tr(ja).to_owned();
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), &v.to_string());
    }
    s
}

/// Localise a kifu library error (the library's own messages are English).
pub fn kifu_error(e: &sakiyomi_kifu::Error) -> String {
    use sakiyomi_kifu::ErrorKind as K;
    let msg = match &e.kind {
        K::IllegalMove(m) => trf("非合法手です: {mv}", &[("mv", m)]),
        K::AmbiguousMove(m) => trf("指し手が曖昧です: {mv}", &[("mv", m)]),
        K::UnparsableMove(m) => trf("指し手を解釈できません: {mv}", &[("mv", m)]),
        K::SameWithoutPrevious(m) => trf("「同」の前に指し手がありません: {mv}", &[("mv", m)]),
        K::InvalidPosition(m) => trf("局面が不正です: {err}", &[("err", m)]),
        K::InvalidSfen(m) => trf("SFENエラー: {err}", &[("err", m)]),
        K::UnsupportedHandicap(m) => trf("未対応の手合割です: {name}", &[("name", m)]),
        K::InvalidHand(m) => trf("持駒を解釈できません: {token}", &[("token", m)]),
        K::InvalidBoardDiagram(m) => trf("盤面図の駒を解釈できません: {piece}", &[("piece", m)]),
        K::InvalidVariation(n) => trf("変化の手数が不正です: {ply}", &[("ply", n)]),
        K::InvalidPieceCode(m) => trf("駒コードが不正です: {code}", &[("code", m)]),
        K::InvalidSquare(m) => trf("升目が不正です: {sq}", &[("sq", m)]),
        K::GameAlreadyOver => tr("終局後には指せません").to_owned(),
        K::TooLarge { what, limit } => trf("{what}が上限（{limit}）を超えています", &[("what", what), ("limit", limit)]),
        K::Corrupt(m) => trf("データが壊れています: {err}", &[("err", m)]),
        K::Unsupported(m) => trf("未対応です: {err}", &[("err", m)]),
        K::UnknownFormat => tr("形式を判別できません").to_owned(),
        other => other.to_string(),
    };
    match e.line {
        Some(l) => trf("{line}行目: {err}", &[("line", &l), ("err", &msg)]),
        None => msg,
    }
}

pub fn book_error(e: &sakiyomi_book::BookError) -> String {
    use sakiyomi_book::BookError as B;
    match e {
        B::Corrupt(m) => trf("定跡ファイルが壊れています: {err}", &[("err", m)]),
        B::TooLarge { what, limit } => trf("{what}が上限（{limit}）を超えています", &[("what", what), ("limit", limit)]),
        B::Io(m) => trf("読み書きできません: {err}", &[("err", m)]),
        B::Unsupported(m) => trf("未対応です: {err}", &[("err", m)]),
    }
}

const EN: &[(&str, &str)] = &[
    ("{what}が上限（{limit}）を超えています", "{what} exceeds the limit ({limit})"),
    ("データが壊れています: {err}", "Corrupt data: {err}"),
    ("未対応です: {err}", "Not supported: {err}"),
    ("形式を判別できません", "Cannot determine the format"),
    ("定跡ファイルが壊れています: {err}", "Corrupt book file: {err}"),
    ("読み書きできません: {err}", "Cannot read or write the file: {err}"),
    ("ファイルが大きすぎます: {path}", "File too large: {path}"),
    ("既に存在します: {path}", "Already exists: {path}"),
    ("出力先がありません: {path}", "Output folder does not exist: {path}"),
    ("中止しました", "Cancelled"),
    ("局面形式を判別できません", "Cannot determine the position format"),
    ("この局面はバイナリ形式にできません: {err}", "This position cannot be stored in a binary format: {err}"),
    ("16進数32バイトではありません", "Not 32 bytes of hexadecimal"),
    ("エンジンの探索に失敗しました: {err}", "Engine search failed: {err}"),
    ("ワーカー数は1以上にしてください", "Use at least one worker"),
    ("入力ファイルがありません", "No input files"),
    // sides and terminal states
    ("先手", "Sente"),
    ("後手", "Gote"),
    ("下手", "Shitate"),
    ("上手", "Uwate"),
    ("あなた", "You"),
    ("投了", "Resign"),
    ("中断", "Interrupted"),
    ("千日手", "Sennichite"),
    ("詰み", "Checkmate"),
    ("持将棋", "Jishogi"),
    ("切れ負け", "Loss on time"),
    ("反則勝ち", "Win by foul"),
    ("反則負け", "Loss by foul"),
    ("入玉勝ち", "Entering-king win"),
    ("{other}の勝ち（{stm}の投了）", "{other} wins ({stm} resigned)"),
    ("{other}の勝ち（詰み）", "{other} wins (checkmate)"),
    ("{other}の勝ち（{stm}の時間切れ）", "{other} wins ({stm} lost on time)"),
    ("{other}の勝ち（{stm}の反則）", "{other} wins ({stm} made an illegal move)"),
    ("{stm}の勝ち（{other}の反則）", "{stm} wins ({other} made an illegal move)"),
    ("{stm}の勝ち（入玉宣言）", "{stm} wins (entering-king declaration)"),
    ("千日手（引き分け）", "Sennichite (draw by repetition)"),
    ("持将棋（引き分け）", "Jishogi (impasse, draw)"),
    ("引き分け（{what}）", "Draw ({what})"),
    ("SPGファイルを読み書きできません: {err}", "Cannot read or write the SPG file: {err}"),
    ("マニフェストは <名前>.spgm です", "A manifest is named <name>.spgm"),
    ("棋譜が含まれていません", "The file holds no game records"),
    ("{total}局のうち{opened}局を開きました", "Opened {opened} of {total} game records"),
    ("表示できない情報があります（保存すると失われます）: {what}", "Some data cannot be shown and would be lost when saving: {what}"),
    ("局面単位の形式から棋譜は作れません", "Game records cannot be made from a position-only format"),
    ("引き分け", "Draw"),
    ("最大手数", "Move limit"),
    ("不詰", "No mate"),
    ("待った", "Take-back"),
    // status lines
    ("対局終了: {text}", "Game over: {text}"),
    ("対局中", "Playing"),
    ("棋譜解析が完了しました", "Game analysis finished"),
    ("棋譜解析中…", "Analysing the game…"),
    ("検討中: {name}", "Analysing: {name}"),
    ("検討エンジン {name} が終了しました", "Analysis engine {name} exited"),
    ("エンジン {name} が異常終了したため対局を中断しました", "Game interrupted: engine {name} exited unexpectedly"),
    ("エンジンを起動しています…", "Starting engines…"),
    ("{name} を起動しています…", "Starting {name}…"),
    // errors
    ("存在しない局面です", "No such position"),
    ("棋譜解析中は移動できません", "You can't move around during game analysis"),
    ("エンジン起動中です", "An engine is starting up"),
    ("対局中は最新の局面でのみ指せます", "During a game you can only move in the latest position"),
    ("エンジンの手番です", "It is the engine's turn"),
    ("非合法手です", "Illegal move"),
    ("非合法手です: {mv}", "Illegal move: {mv}"),
    ("エンジンが選択されていません", "No engine selected"),
    ("登録されていないエンジンです", "That engine is not registered"),
    ("開始局面が不正です: {err}", "Invalid start position: {err}"),
    ("対局中です。先に中断してください", "A game is in progress; stop it first"),
    ("{name} を起動できません: {err}", "Cannot start {name}: {err}"),
    ("セッションは閉じられました", "The session was closed"),
    ("対局中ではありません", "No game in progress"),
    ("投了は自分の手番で行ってください", "You can only resign on your own turn"),
    ("人間の手番です", "It is a human player's turn"),
    ("対局中は検討できません", "You can't analyse during a game"),
    ("対局中は変更できません", "You can't change this during a game"),
    ("読み込めません: {err}", "Cannot read the file: {err}"),
    ("未対応の形式です", "Unsupported format"),
    ("保存できません: {err}", "Cannot save the file: {err}"),
    ("開始局面は削除できません", "The start position can't be deleted"),
    ("棋譜解析中は削除できません", "You can't delete moves during game analysis"),
    ("削除できません", "Cannot delete"),
    ("セッション {id} がありません", "Session {id} does not exist"),
    ("エンジンが見つかりません", "Engine not found"),
    ("起動できません: {err}", "Cannot start: {err}"),
    ("エンジンの入出力エラー: {err}", "Engine I/O error: {err}"),
    ("エンジンが応答しません ({what})", "The engine is not responding ({what})"),
    ("エンジンが終了しました", "The engine exited"),
    ("SFENが短すぎます", "The SFEN is too short"),
    ("局面文字列を解釈できません", "Cannot parse the position string"),
    ("SFENエラー: {err}", "SFEN error: {err}"),
    ("終局後には指せません", "The game is over; no more moves can be played"),
    ("局面が不正です: {err}", "Invalid position: {err}"),
    ("未対応の手合割です: {name}", "Unsupported handicap: {name}"),
    ("持駒を解釈できません: {token}", "Cannot parse pieces in hand: {token}"),
    ("持駒の数を解釈できません: {token}", "Cannot parse the number of pieces in hand: {token}"),
    ("持駒にできない駒です: {token}", "That piece can't be held in hand: {token}"),
    ("持駒にできない駒です", "That piece can't be held in hand"),
    ("盤面図の行が短すぎます", "A board-diagram row is too short"),
    ("盤面図の駒を解釈できません: {piece}", "Cannot parse a piece in the board diagram: {piece}"),
    ("変化の手数が不正です: {ply}", "Invalid variation move number: {ply}"),
    ("{line}行目: {err}", "Line {line}: {err}"),
    ("駒コードが不正です: {code}", "Invalid piece code: {code}"),
    ("升目が不正です: {sq}", "Invalid square: {sq}"),
    ("指し手が不正です: {mv}", "Invalid move: {mv}"),
    ("「同」の前に指し手がありません: {mv}", "\"同\" (same square) without a previous move: {mv}"),
    ("指し手を解釈できません: {mv}", "Cannot parse the move: {mv}"),
    ("駒の種類を解釈できません: {mv}", "Cannot parse the piece type: {mv}"),
    ("指し手が曖昧です: {mv}", "Ambiguous move: {mv}"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `tr("…")` / `trf("…", …)` key in the crate has an English entry.
    #[test]
    fn every_key_is_translated() {
        let sources = [include_str!("session.rs"), include_str!("tools.rs"), include_str!("searcher.rs"), include_str!("i18n.rs")];
        let mut missing = Vec::new();
        let mut count = 0;
        for full in sources {
            // Code only: drop tests and doc comments (which contain examples).
            let code: String = full
                .split("#[cfg(test)]")
                .next()
                .unwrap_or("")
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("
");
            let src = code.as_str();
            for pat in ["tr(\"", "trf(\""] {
                for (i, _) in src.match_indices(pat) {
                    // Skip e.g. push_str("…"): the call must start at a word boundary.
                    let prev = src[..i].chars().last().unwrap_or(' ');
                    if prev.is_alphanumeric() || prev == '_' {
                        continue;
                    }
                    let rest = &src[i + pat.len()..];
                    let key = &rest[..rest.find('"').unwrap()];
                    count += 1;
                    if !EN.iter().any(|(k, _)| *k == key) {
                        missing.push(key.to_owned());
                    }
                }
            }
        }
        assert!(count > 50, "expected many translated messages, found {count}");
        assert!(missing.is_empty(), "missing English for: {missing:?}");
        // Placeholders survive translation.
        for (ja, en) in EN {
            for p in ["{name}", "{err}", "{text}", "{other}", "{stm}", "{mv}", "{line}", "{id}"] {
                assert_eq!(ja.contains(p), en.contains(p), "{ja} / {en}");
            }
        }
    }

    #[test]
    fn switching_language() {
        set_lang("en-US");
        assert_eq!(tr("投了"), "Resign");
        assert_eq!(trf("{line}行目: {err}", &[("line", &3), ("err", &"x")]), "Line 3: x");
        assert_eq!(tr("未登録のキー"), "未登録のキー");
        set_lang("ja");
        assert_eq!(trf("{line}行目: {err}", &[("line", &3), ("err", &"x")]), "3行目: x");
    }
}
