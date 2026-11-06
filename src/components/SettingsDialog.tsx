import { setSettingsOpen, updateSettings, useStore } from "../store";
import {
  ACCENTS, BOARDS, DEFAULT_SETTINGS, PIECE_FONTS, PieceLabelContext, PieceLabels, ThemeMode, resolvedTheme,
} from "../settings";
import { PieceGlyph } from "./Board";
import { t } from "../i18n";
import type { LanguageSetting, NotationSetting } from "../settings";
import type { EvalStyle } from "../api";
import { Modal } from "./Dialogs";

function Segmented<T extends string>({ value, options, onChange }: { value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <div className="segmented" role="radiogroup">
      {options.map(([v, label]) => (
        <button key={v} role="radio" aria-checked={v === value} className={v === value ? "on" : ""} onClick={() => onChange(v)}>
          {label}
        </button>
      ))}
    </div>
  );
}

const pct = (x: number) => `${Math.round(x * 100)}%`;

export function SettingsDialog() {
  const s = useStore((st) => st.settings);
  const theme = resolvedTheme(s);
  const close = () => setSettingsOpen(false);

  return (
    <Modal title={t("設定")} onClose={close} wide>
      <div className="settings">
        <section>
          <h3>{t("言語と表記")}</h3>
          <span className="label">{t("言語")}</span>
          <Segmented<LanguageSetting>
            value={s.language}
            options={[["system", t("システムの言語")], ["ja", "日本語"], ["en", "English"]]}
            onChange={(language) => updateSettings({ language })}
          />
          <span className="label">{t("棋譜表記")}</span>
          <Segmented<NotationSetting>
            value={s.notation}
            options={[["auto", t("言語に合わせる")], ["ja", t("日本語（▲７六歩）")], ["western", t("西洋式（☗P-7f）")], ["usi", t("USI（7g7f）")]]}
            onChange={(notation) => updateSettings({ notation })}
          />
          <span />
          <span className="muted small">{t("棋譜ファイル（KIF/KI2/CSA）は表記の設定に関係なく標準形式で保存されます。")}</span>
          <span className="label">{t("評価値・読み筋の保存形式")}</span>
          <Segmented<EvalStyle>
            value={s.evalStyle}
            options={[["shogigui", "ShogiGUI"], ["shogihome", "ShogiHome"], ["floodgate", "Floodgate (CSA)"], ["none", t("保存しない")]]}
            onChange={(evalStyle) => updateSettings({ evalStyle })}
          />
          <span />
          <span className="muted small">{t("読み込み時はどの形式も自動で認識します。")}</span>
        </section>
        <section>
          <h3>{t("外観")}</h3>
          <span className="label">{t("テーマ")}</span>
          <Segmented<ThemeMode>
            value={s.theme}
            options={[["system", t("システムに合わせる")], ["dark", t("ダーク")], ["light", t("ライト")]]}
            onChange={(theme) => updateSettings({ theme })}
          />
          <span className="label">{t("アクセントカラー")}</span>
          <div className="swatches">
            {Object.entries(ACCENTS).map(([k, a]) => (
              <button key={k} className={"swatch" + (s.accent === k ? " on" : "")} style={{ background: a[theme] }}
                title={t(a.name)} aria-label={t(a.name)} onClick={() => updateSettings({ accent: k })} />
            ))}
          </div>
          <span className="label">{t("盤の色")}</span>
          <div className="swatches">
            {Object.entries(BOARDS).map(([k, b]) => (
              <button key={k} className={"board-swatch" + (s.board === k ? " on" : "")}
                style={{ background: `linear-gradient(135deg, ${b.a}, ${b.b})` }}
                title={t(b.name)} onClick={() => updateSettings({ board: k })}>
                <span>{t(b.name)}</span>
              </button>
            ))}
          </div>
        </section>

        <section>
          <h3>{t("駒")}</h3>
          <span className="label">{t("駒の文字")}</span>
          <div className="slider-row">
            <Segmented<PieceLabels>
              value={s.pieceLabels}
              options={[["one", t("1文字（歩・龍）")], ["two", t("2文字（歩兵・龍王）")]]}
              onChange={(pieceLabels) => updateSettings({ pieceLabels })}
            />
            <PieceLabelContext.Provider value={s.pieceLabels}>
              <div className="preview-pieces" aria-hidden>
                {[6, 14, 8, 3].map((k) => (
                  <svg key={k} viewBox="0 0 60 60"><PieceGlyph kind={k} color={0} x={0} y={0} down={false} /></svg>
                ))}
              </div>
            </PieceLabelContext.Provider>
          </div>
          <span className="label">{t("駒の書体")}</span>
          <Segmented
            value={s.pieceFont}
            options={Object.entries(PIECE_FONTS).map(([k, f]) => [k, t(f.name)] as [string, string])}
            onChange={(pieceFont) => updateSettings({ pieceFont })}
          />
        </section>

        <section>
          <h3>{t("文字と表示サイズ")}</h3>
          <span className="label">{t("文字サイズ")}</span>
          <div className="slider-row">
            <input type="range" min={0.8} max={1.6} step={0.05} value={s.fontScale}
              onChange={(e) => updateSettings({ fontScale: Number(e.target.value) })} aria-label={t("文字サイズ")} />
            <output>{pct(s.fontScale)}</output>
          </div>
          <span className="label">{t("表示サイズ（ズーム）")}</span>
          <div className="slider-row">
            <input type="range" min={0.5} max={2} step={0.05} value={s.zoom}
              onChange={(e) => updateSettings({ zoom: Number(e.target.value) })} aria-label={t("表示サイズ")} />
            <output>{pct(s.zoom)}</output>
          </div>
          <span />
          <span className="muted small">
            {t("Ctrl+= で拡大、Ctrl+- で縮小、Ctrl+0 で100%に戻します。文字サイズはメニューや棋譜・読み筋の文字だけを変え、表示サイズは盤を含む画面全体を拡大縮小します。")}
          </span>
        </section>
      </div>
      <div className="dialog-actions">
        <button onClick={() => updateSettings({ ...DEFAULT_SETTINGS })}>{t("初期設定に戻す")}</button>
        <span className="spacer" />
        <button className="primary" onClick={close}>{t("閉じる")}</button>
      </div>
    </Modal>
  );
}
