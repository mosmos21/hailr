# hailr

herdr の pane で動いている Claude Code の質問と権限確認を、1 つの TUI に集めて答える道具。
Claude は `ask_human` ツールで質問を出したらターンを終え、TUI で押した回答が channel で Claude に届く。

## 必要なもの

- herdr
- Rust（stable）
- Node 26 以降と pnpm
- Claude Code（channel を使うため、claude.ai か Console の API キーでログインしていること）

## 導入

リポジトリのルートで実行する。

1. TUI を入れる

   ```sh
   cargo install --path crates/hailr-tui
   ```

2. ask-mcp の依存を入れる

   ```sh
   pnpm install
   ```

3. ask-mcp を Claude Code に `hailr` という名前で登録する

   ```sh
   claude mcp add --scope user hailr -- node "$PWD/packages/ask-mcp/src/main.ts"
   ```

## 使い方

1. herdr の pane で `hailr` を起動する
2. 別の pane で Claude Code を channel 付きで起動する。開発用 channel の警告が出るので、ローカルでの開発に使う旨の選択肢を選ぶ

   ```sh
   claude --dangerously-load-development-channels server:hailr
   ```

3. Claude が `ask_human` を呼ぶと、TUI に質問が並ぶ。端末に権限確認のダイアログが出るときは、同じものが Allow / Deny の質問として並び、先に答えたほうが使われる

| キー | すること |
|---|---|
| `↑` `↓`（`k` `j`） | 質問を選ぶ |
| `1`〜`9` | ボタンで答える |
| `o` のあと数字 | リンクを開く |
| `Enter` | その pane に入る（`herdr agent attach`） |
| `d` | 質問を消す（Claude には答えずに消したと伝わる。権限確認は何も伝わらず、端末のダイアログが残る） |
| `q` | 終わる |

質問・ボタン・リンクはクリックでも選べる。

質問の一覧は既定で右に出る。左にしたいときは `~/.config/hailr/config.toml` に書く。

```toml
[ui]
questions = "left"
```

新しい質問が届いても、既定では herdr の通知を出さない。出したいときは同じファイルに書く。

```toml
[notify]
toast = true
```

ボタンで答えた質問は 1.5 秒残る。長さは秒で変えられ（1 時間まで）、0 ならすぐ消える。

```toml
[ui]
answered_delay = 0.5
```
