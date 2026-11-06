// Book panel: moves stored in an opened book for the displayed position. Click a move to play it.
import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, BookProbeView, SessionView } from "../api";
import { closeBook, run, toast, useStore } from "../store";
import { t } from "../i18n";

const MARKS: Record<string, string> = { forced: "!!", good: "!", bad: "?", blunder: "??", none: "" };

export function BookView({ v, onPlay }: { v: SessionView; onPlay: (usi: string) => void }) {
  const books = useStore((s) => s.books);
  const [selected, setSelected] = useState<number | null>(null);
  const [probe, setProbe] = useState<BookProbeView | null>(null);
  const book = books.find((b) => b.id === selected) ?? books[books.length - 1] ?? null;
  const sfen = v.board.sfen;

  useEffect(() => {
    let live = true;
    if (!book) {
      setProbe(null);
      return;
    }
    api.probeBook(book.id, sfen).then((p) => live && setProbe(p)).catch(() => live && setProbe(null));
    return () => {
      live = false;
    };
  }, [book?.id, sfen]);

  const openBook = async () => {
    const p = await open({ multiple: false, filters: [{ name: t("定跡"), extensions: ["spg", "spgt", "spgm", "db", "sbk", "bin"] }, { name: t("すべて"), extensions: ["*"] }] });
    if (typeof p === "string" && (await run(api.jobOpenBook(p))) !== undefined) toast(t("定跡を読み込んでいます…"));
  };

  return (
    <div className="bookview">
      <div className="bookview-head">
        <select value={book?.id ?? ""} onChange={(e) => setSelected(Number(e.target.value))} disabled={!books.length}>
          {!books.length && <option value="">{t("定跡が開かれていません")}</option>}
          {books.map((b) => <option key={b.id} value={b.id} title={b.path}>{b.name}</option>)}
        </select>
        <button onClick={openBook}>{t("開く…")}</button>
        {book && <button className="icon" title={t("この定跡を閉じる")} onClick={() => { closeBook(book.id); setSelected(null); }}>✕</button>}
      </div>
      {book && probe && !probe.found && <p className="muted small">{t("この局面は定跡にありません")}</p>}
      {probe?.found && (
        <table className="bookview-moves">
          <thead>
            <tr><th>{t("指し手")}</th><th>{t("評価値")}</th><th>{t("深さ")}</th><th>{t("回数")}</th><th /></tr>
          </thead>
          <tbody>
            {probe.moves.map((m) => (
              <tr key={m.usi} onClick={() => onPlay(m.usi)} title={t("クリックでこの手を指す")}>
                <td>{m.label}<span className="mark">{MARKS[m.mark]}</span></td>
                <td className={"num " + ((m.value ?? 0) >= 0 ? "pos" : "neg")}>{m.value ?? "—"}</td>
                <td className="num">{m.depth ?? ""}</td>
                <td className="num">{m.count}</td>
                <td className="share"><div style={{ width: `${Math.round(m.share * 100)}%` }} /></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {probe?.flipped && <p className="muted small">{t("先後反転した局面の定跡を表示しています")}</p>}
      {probe?.comment && <p className="bookview-comment">{probe.comment}</p>}
    </div>
  );
}
