//! Kifu as written by other GUIs (ShogiGUI, Kifu for Windows, CSA V3.0 servers),
//! including Shift_JIS encoding, evaluation comments, bookmarks and handicaps.

use sakiyomi_kifu::format::{self, Format, TextEncoding};
use sakiyomi_kifu::{GameRecord, Limits, Special};

/// ShogiGUI-style KIF with analysis comments, saved as Shift_JIS with CRLF.
const SHOGIGUI_KIF: &str = "\
# ---- ShogiGUI ----
開始日時：2024/03/10 14:00:00
終了日時：2024/03/10 14:05:12
手合割：平手
先手：Engine A
後手：Engine B
手数----指手---------消費時間--
   1 ２六歩(27)   ( 0:01/00:00:01)
**解析 0 ○ 候補手1 時間 00:01.0 深さ 20/28 ノード数 1234567 評価値 45 読み筋 ▲２六歩(27) △３四歩(33)
   2 ３四歩(33)   ( 0:02/00:00:02)
&ブックマーク1
   3 ７六歩(77)   ( 0:01/00:00:02)
   4 ４四歩(43)   ( 0:03/00:00:05)
   5 ４八銀(39)   ( 0:01/00:00:03)
   6 ３二飛(82)   ( 0:02/00:00:07)
   7 ６八玉(59)   ( 0:01/00:00:04)
   8 ６二玉(51)   ( 0:01/00:00:08)
   9 ７八玉(68)   ( 0:01/00:00:05)
  10 ７二玉(62)   ( 0:01/00:00:09)
  11 中断         ( 0:00/00:00:05)
まで10手で中断
";

/// Kifu for Windows branch file (UTF-8 .kifu) with a handicap and bookmarked variation.
const KFW_HANDICAP: &str = "\
#KIF version=2.0 encoding=UTF-8
手合割：香落ち
上手：上手さん
下手：下手さん
手数----指手---------消費時間--
   1 ３四歩(33)   ( 0:00/00:00:00)
   2 ７六歩(77)   ( 0:00/00:00:00)+
   3 ８四歩(83)   ( 0:00/00:00:00)
   4 投了   ( 0:00/00:00:00)
まで3手で上手の勝ち

変化：2手
   2 ２六歩(27)   ( 0:00/00:00:00)
*居飛車
";

/// CSA V3.0 as produced by Floodgate-style servers.
const CSA_V3: &str = "\
'CSA encoding=SHIFT_JIS
V3.0
N+sente_engine
N-gote_engine
$EVENT:wdoor+floodgate-300-10F+test
$START_TIME:2024/01/01 00:00:00
$TIME:300+00+10
P1-KY-KE-GI-KI-OU-KI-GI-KE-KY
P2 * -HI *  *  *  *  * -KA *
P3-FU-FU-FU-FU-FU-FU-FU-FU-FU
P4 *  *  *  *  *  *  *  *  *
P5 *  *  *  *  *  *  *  *  *
P6 *  *  *  *  *  *  *  *  *
P7+FU+FU+FU+FU+FU+FU+FU+FU+FU
P8 * +KA *  *  *  *  * +HI *
P9+KY+KE+GI+KI+OU+KI+GI+KE+KY
+
+2726FU,T1.25
'** 30 -8384FU +2625FU
-8384FU,T2
+2625FU,T0.5
-8485FU,T3
+6978KI,T1
-4132KI,T1
%TORYO,T5
";

#[test]
fn shogigui_shift_jis_kif() {
    let bytes = format::encode(SHOGIGUI_KIF, TextEncoding::ShiftJis);
    let rec = format::read(&format::decode(&bytes), None, &Limits::default()).unwrap();
    assert_eq!(rec.black_name(), Some("Engine A"));
    let main = rec.mainline();
    assert_eq!(main.len(), 12);
    assert_eq!(rec.nodes[main[11]].special, Some(Special::Interrupt));
    // The ShogiGUI analysis line becomes structured data.
    assert_eq!(rec.nodes[main[1]].analysis.analysis.as_ref().and_then(|a| a.eval).map(|e| e.cp), Some(45));
    assert_eq!(rec.nodes[main[10]].label, "△７二玉");
    // Written back out, it parses to the same moves in every format.
    for f in [Format::Kif, Format::Ki2, Format::Csa] {
        let text = format::write(&rec, f, TextEncoding::ShiftJis, None).unwrap();
        let again = format::read(&text, Some(f), &Limits::default()).unwrap();
        let labels = |r: &GameRecord| r.mainline().iter().map(|&n| r.nodes[n].label.clone()).collect::<Vec<_>>();
        assert_eq!(labels(&again), labels(&rec), "{f:?}");
    }
}

#[test]
fn kifu_for_windows_handicap_with_variation() {
    let rec = format::read(KFW_HANDICAP, None, &Limits::default()).unwrap();
    assert!(rec.is_handicap());
    assert_eq!(rec.white_name(), Some("上手さん"));
    assert_eq!(rec.initial().side, 1);
    let first = rec.nodes[0].children[0];
    assert_eq!(rec.nodes[first].label, "△３四歩");
    assert_eq!(rec.nodes[first].children.len(), 2);
    let var = rec.nodes[first].children[1];
    assert_eq!(rec.nodes[var].label, "▲２六歩");
    assert_eq!(rec.nodes[var].comment, "居飛車");
    let text = format::write(&rec, Format::Kif, TextEncoding::Utf8, None).unwrap();
    assert!(text.contains("手合割：香落ち"));
    assert!(text.contains("まで3手で上手の勝ち"));
    assert!(text.contains("変化：2手"));
}

#[test]
fn csa_v3_floodgate() {
    let bytes = format::encode(CSA_V3, TextEncoding::ShiftJis);
    let rec = format::read(&format::decode(&bytes), None, &Limits::default()).unwrap();
    assert_eq!(rec.black_name(), Some("sente_engine"));
    assert_eq!(rec.header("棋戦"), Some("wdoor+floodgate-300-10F+test"));
    let main = rec.mainline();
    assert_eq!(main.len(), 8);
    assert_eq!(rec.nodes[main[1]].time_ms, Some(1250));
    assert_eq!(rec.nodes[main[5]].label, "▲７八金");
    assert_eq!(rec.nodes[main[7]].special, Some(Special::Resign));
    // Floodgate evaluation comment becomes Sente's engine reading.
    assert_eq!(rec.nodes[main[1]].analysis.black.as_ref().and_then(|a| a.eval).map(|e| e.cp), Some(30));
    let kif = format::write(&rec, Format::Kif, TextEncoding::ShiftJis, None).unwrap();
    // Black is to move after White's 6th move, so %TORYO is Black resigning.
    assert!(kif.contains("まで6手で後手の勝ち"), "{kif}");
}
