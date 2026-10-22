import path from "node:path";

type Env = Record<string, string | undefined>;

/** broker と同じ規則で決める。相対パスだと broker と作業ディレクトリが違えば別の場所を指すため、絶対パスのときだけ使う */
export function socketPath(env: Env): string | undefined {
  const absolute = (value: string | undefined) =>
    value && path.isAbsolute(value) ? value : undefined;
  const home = absolute(env.HOME);
  const base =
    absolute(env.XDG_STATE_HOME) ??
    (home ? path.join(home, ".local", "state") : undefined);
  return base && path.join(base, "hailr", "broker.sock");
}
