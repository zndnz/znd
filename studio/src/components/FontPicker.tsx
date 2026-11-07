// フォントの選択欄。インストール済みのフォントを検索でき（日本語名などの別名でも引ける）、
// 一覧の各行はそのフォントで本文の見本を描く（WebView が OS のフォントをそのまま使う）。

import { useEffect, useState } from "react";
import { api } from "../api";
import { Button } from "@/components/ui/button";
import { Command, CommandEmpty, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Icon } from "./icons";

let fontsCache: Promise<[string, string[]][]> | null = null;
const loadFonts = () => (fontsCache ??= api.fonts());

/** 本文が空のときの見本（かな・漢字・英字を混ぜて、和文か欧文かが分かるように）。 */
const DEFAULT_SAMPLE = "あア永 Aa";

/** 見本にする文字列（本文の最初の 1 行・最大 16 文字）。 */
const sampleOf = (text: string) => text.split("\n").map((l) => l.trim()).find(Boolean)?.slice(0, 16) || DEFAULT_SAMPLE;

export function FontPicker({ value, text, onChange }: { value: string; text: string; onChange: (family: string) => void }) {
  const [open, setOpen] = useState(false);
  const [fonts, setFonts] = useState<[string, string[]][]>([]);
  useEffect(() => {
    if (open) void loadFonts().then(setFonts);
  }, [open]);
  const sample = sampleOf(text);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger render={<Button variant="secondary" size="sm" className="w-full min-w-0 justify-between rounded-3xl font-normal" />}>
        <span className="truncate" style={{ fontFamily: value ? `"${value}"` : undefined }}>
          {value || "既定のフォント"}
        </span>
        <Icon name="chevron" size={14} className="text-muted-foreground" />
      </PopoverTrigger>
      <PopoverContent align="end" className="w-96 p-0">
        <Command>
          <CommandInput placeholder="フォントを検索（日本語名でも）" onKeyDown={(e) => e.stopPropagation()} />
          <CommandList className="max-h-96">
            <CommandEmpty>{fonts.length ? "一致するフォントがありません" : "読み込み中…"}</CommandEmpty>
            <CommandItem value="既定のフォント default" onSelect={() => (setOpen(false), onChange(""))}>
              <span className="flex-1">既定のフォント</span>
              <span className="text-muted-foreground">{sample}</span>
            </CommandItem>
            {fonts.map(([family, aliases]) => (
              <CommandItem key={family} value={[family, ...aliases].join(" ")} onSelect={() => (setOpen(false), onChange(family))}>
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-base" style={{ fontFamily: `"${family}"` }}>
                    {sample}
                  </span>
                  <span className="truncate text-xs text-muted-foreground">
                    {family}
                    {aliases.length > 0 && `（${aliases[0]}）`}
                  </span>
                </span>
                {family === value && <Icon name="diamond" size={10} filled className="text-muted-foreground" />}
              </CommandItem>
            ))}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
