"""Regenerate the offline kaomoji picker; run explicitly, never during a build."""
import json
import urllib.request
from pathlib import Path

REVISION = "60c92ea4e85279ad42ff525e9a40464ebeb1003e"
BASE = f"https://raw.githubusercontent.com/kaomojikan/kaomoji-data/{REVISION}/"
ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "assets" / "emoji"
# IDs and order match src/core/kaomoji.rs. Aliases supplement upstream Japanese tags.
CATEGORIES = [
    ("cute", "可爱", "Cute", "萌 萌萌 哇 卡哇伊 kawaii adorable sweet"),
    ("cry", "难过与哭泣", "Sad & crying", "难过 伤心 哭 哭泣 眼泪 委屈 sad crying tears sob"),
    ("nadenade", "摸摸头", "Headpats", "摸头 安慰 抚摸 抱抱 headpat comfort soothe"),
    ("chira", "偷看", "Peeking", "偷看 围观 探头 躲 peek glance wall"),
    ("copype", "经典", "Classics", "经典 常用 无奈 耸肩 classic popular shrug"),
    ("nageru", "掀桌与投掷", "Table flips & throws", "掀桌 扔 丢 投掷 生气 table flip throw toss angry"),
    ("mogumogu", "吃吃喝喝", "Food & drink", "吃 喝 好吃 美味 流口水 eat drink hungry yummy nom"),
    ("animal", "动物", "Animals", "动物 猫 狗 熊 兔 猪 鸟 animal cat dog bear rabbit pig bird"),
    ("ureshii", "开心", "Happy", "开心 高兴 快乐 兴奋 欢呼 happy joy excited yay"),
    ("tereru", "害羞", "Shy", "害羞 脸红 腼腆 shy blush bashful"),
    ("bikkuri", "惊讶", "Surprised", "惊讶 震惊 吃惊 吓 惊呆 surprise shock wow"),
    ("emoji", "表情混搭", "Emoji mix", "表情 混搭 装饰 emoji mix decorated"),
    ("ryosangata", "软萌", "Soft & cute", "软萌 甜美 少女 可爱 soft cute dreamy"),
    ("egao", "笑脸", "Smiling", "笑 微笑 大笑 哈哈 smile laugh happy"),
    ("kirakira", "闪闪发光", "Sparkles", "闪亮 闪闪 星星 发光 sparkle star glitter shine"),
    ("otaku", "宅与追星", "Fandom", "宅 追星 应援 激动 otaku fan fandom"),
    ("suki", "喜欢与爱", "Love", "喜欢 爱 爱心 亲亲 心动 love heart kiss hug"),
    ("tehepero", "吐舌与卖萌", "Playful", "吐舌 卖萌 调皮 眨眼 playful tongue wink tehepero"),
    ("taoreru", "倒地", "Falling over", "倒地 晕倒 趴 累 躺 fall faint tired exhausted"),
    ("aa", "文字画", "ASCII art", "文字画 艺术 论坛 ascii art text 2ch"),
    ("nemui", "困困", "Sleepy", "困 睡觉 瞌睡 晚安 哈欠 sleepy sleep yawn goodnight"),
    ("okoru", "生气", "Angry", "生气 愤怒 不满 哼 angry anger annoyed mad"),
    ("ouen", "加油", "Cheering", "加油 支持 鼓励 应援 cheer support encouragement"),
    ("greetings", "打招呼", "Greetings", "你好 再见 挥手 招呼 hello hi goodbye wave greetings"),
]


def fetch(name):
    with urllib.request.urlopen(BASE + name, timeout=30) as response:
        return response.read()


def main():
    source = json.loads(fetch("kaomoji.json"))
    metadata = json.loads(fetch("categories.json"))
    license_text = fetch("LICENSE").decode("utf-8")
    ids = {category[0]: index for index, category in enumerate(CATEGORIES)}
    merged = {}
    skipped = 0
    for entry in source:
        text = entry["text"]
        # The compact single-line picker cannot display multiline ASCII pictures.
        # Preserve spaces and Unicode sequences in every accepted expression.
        if not text.strip() or any(ord(c) < 32 or ord(c) == 127 for c in text):
            skipped += 1
            continue
        row = merged.setdefault(text, {"groups": set(), "keywords": set()})
        for slug in entry["categories"]:
            row["groups"].add(ids[slug])
            row["keywords"].update(metadata[slug].get("synonyms", []))
        row["keywords"].update(entry.get("tags", []))
        row["keywords"].update(entry.get("reading", []))
    for entry in json.loads((DEST / "kaomoji-curated.json").read_text(encoding="utf-8")):
        row = merged.setdefault(entry["text"], {"groups": set(), "keywords": set()})
        row["groups"].add(ids[entry["category"]])
    rows = []
    for text, entry in merged.items():
        groups = sorted(entry["groups"])
        zh, en = [], []
        for group in groups:
            _, name, name_en, aliases = CATEGORIES[group]
            zh.append(name)
            en.append(name_en)
            entry["keywords"].add(aliases)
        rows.append(dict(text=text, name=" / ".join(zh), name_en=" / ".join(en),
                         keywords=" ".join(sorted(entry["keywords"])),
                         group=groups[0], groups=groups))
    assert len(rows) > 1500
    assert len({row["text"] for row in rows}) == len(rows)
    assert set(range(len(CATEGORIES))) == {g for row in rows for g in row["groups"]}
    (DEST / "kaomoji.json").write_text(
        json.dumps(rows, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    (DEST / "LICENSE-kaomojikan").write_text(license_text, encoding="utf-8")
    print(f"Generated {len(rows)} unique entries; skipped {skipped} multiline/control entries")


if __name__ == "__main__":
    main()
