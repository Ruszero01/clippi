"""Generate offline symbols from Unicode 17.0 and CLDR 48; run explicitly."""
import collections
import json
import urllib.request
import urllib.error
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VERSION = "17.0.0"
UCD_URL = f"https://www.unicode.org/Public/{VERSION}/ucd/UnicodeData.txt"
UCD_MIRROR = f"https://raw.githubusercontent.com/unicode-org/unicodetools/main/unicodetools/data/ucd/{VERSION}/UnicodeData.txt"
CLDR_URL = "https://raw.githubusercontent.com/unicode-org/cldr/release-48/common/annotations/zh.xml"

# IDs match src/core/symbols.rs. Common is a virtual view over these unique rows.
GROUPS = [
    (4, "单位", "Units", [(0x3300, 0x33FF)], "℃℉°′″‴％%‰‱µ"),
    (5, "货币", "Currency", [(0x20A0, 0x20CF)], "$¢£¤¥＄￠￡￥￦"),
    (12, "希腊字母", "Greek letters", [(0x370, 0x3FF), (0x1F00, 0x1FFF)], ""),
    (11, "数字", "Numbers", [(0x2070, 0x209F), (0x2150, 0x218F), (0x2460, 0x24FF), (0x2776, 0x2793), (0x1F100, 0x1F10C)], "²³¹"),
    (1, "箭头", "Arrows", [(0x2190, 0x21FF), (0x27F0, 0x27FF), (0x2900, 0x297F), (0x1F800, 0x1F8FF)], ""),
    (2, "数学", "Math", [(0x2200, 0x22FF), (0x27C0, 0x27EF), (0x2980, 0x29FF), (0x2A00, 0x2AFF)], "+−±×÷=<>¬|~"),
    (3, "标点", "Punctuation", [(0x2000, 0x206F), (0x2E00, 0x2E7F), (0x3000, 0x303F), (0xFF01, 0xFF65)], "!\"#&'()*,-./:;?@[\\]^_`{}§©®™·"),
    (6, "几何图形", "Geometric shapes", [(0x25A0, 0x25FF), (0x1F780, 0x1F7FF)], ""),
    (7, "制表线", "Box drawing", [(0x2500, 0x259F)], ""),
    (8, "杂项", "Miscellaneous", [(0x2600, 0x26FF), (0x2700, 0x27BF), (0x1F700, 0x1F77F)], ""),
    (9, "技术", "Technical", [(0x2300, 0x245F)], ""),
    (10, "字母类", "Letterlike", [(0x2100, 0x214F), (0x1D400, 0x1D7FF)], ""),
]

ALIASES = {
    "$": "美元 美金 dollar", "¢": "美分 cent", "£": "英镑 pound", "¥": "人民币 日元 元 日圆 yen yuan rmb cny jpy",
    "￥": "人民币 日元 元 日圆 yen yuan rmb cny jpy", "€": "欧元 euro", "₩": "韩元 won", "₽": "卢布 ruble",
    "₹": "印度卢比 rupee", "₿": "比特币 bitcoin", "₫": "越南盾 dong", "฿": "泰铢 baht",
    "℃": "摄氏度 摄氏温度 度 celsius", "℉": "华氏度 华氏温度 度 fahrenheit", "°": "度 角度 degree",
    "Ω": "欧姆 电阻 omega ohm", "μ": "微 缪 mu micro", "µ": "微 micro", "%": "百分号 百分比 percent",
    "‰": "千分号 per mille", "‱": "万分号 basis point", "㎡": "平方米 面积 square metre meter",
    "㎥": "立方米 体积 cubic metre meter", "㎏": "千克 公斤 kilogram kg", "㎝": "厘米 centimeter cm",
    "㎜": "毫米 millimeter mm", "㎞": "千米 公里 kilometer km", "㎎": "毫克 milligram mg",
    "±": "正负 加减 plus minus", "×": "乘号 乘法 multiplication", "÷": "除号 除法 division",
    "≤": "小于等于 小于或等于 less than or equal", "≥": "大于等于 大于或等于 greater than or equal",
    "≠": "不等于 不等号 not equal", "≈": "约等于 近似 approximately", "∞": "无穷 无穷大 无限 infinity",
    "√": "平方根 根号 square root", "∑": "求和 总和 summation", "∫": "积分 integral",
    "∈": "属于 element of", "∉": "不属于 not an element", "∅": "空集 empty set", "∩": "交集 intersection", "∪": "并集 union",
    "✓": "对勾 勾 正确 check", "✔": "对勾 勾 正确 check", "✕": "叉 错误 cross", "✖": "叉 错误 cross",
    "←": "左箭头 向左 left arrow", "→": "右箭头 向右 right arrow", "↑": "上箭头 向上 up arrow", "↓": "下箭头 向下 down arrow",
    "©": "版权 copyright", "®": "注册商标 registered", "™": "商标 trademark", "…": "省略号 ellipsis",
}
NAMES = {
    "$": "美元符号", "¢": "美分符号", "£": "英镑符号", "¥": "人民币／日元符号", "￥": "全角人民币／日元符号",
    "€": "欧元符号", "₩": "韩元符号", "₽": "卢布符号", "₹": "印度卢比符号", "₿": "比特币符号",
    "℃": "摄氏度", "℉": "华氏度", "°": "度", "㎡": "平方米", "㎥": "立方米", "㎏": "千克",
    "≤": "小于等于", "≥": "大于等于", "≠": "不等于", "≈": "约等于", "∞": "无穷大", "√": "平方根",
    "∑": "求和", "∫": "积分", "±": "正负号", "×": "乘号", "÷": "除号", "✓": "对勾", "✔": "粗对勾",
}
TOKENS = {
    "LEFT": "左", "LEFTWARDS": "向左 左箭头", "RIGHT": "右", "RIGHTWARDS": "向右 右箭头",
    "UP": "上", "UPWARDS": "向上 上箭头", "DOWN": "下", "DOWNWARDS": "向下 下箭头", "ARROW": "箭头",
    "CIRCLE": "圆 圆形", "CIRCLED": "带圈 圆圈 圈号", "SQUARE": "方形 方框", "SQUARED": "带框",
    "BLACK": "实心 黑", "WHITE": "空心 白", "TRIANGLE": "三角形", "DIAMOND": "菱形", "STAR": "星形 星星",
    "BOX": "方框 制表", "DRAWINGS": "制表线 边框", "LIGHT": "细线", "HEAVY": "粗线", "DOUBLE": "双线 双",
    "HORIZONTAL": "水平 横线", "VERTICAL": "垂直 竖线", "BLOCK": "块 方块", "SHADE": "阴影",
    "PLUS": "加 正", "MINUS": "减 负", "EQUAL": "等于", "LESS-THAN": "小于", "GREATER-THAN": "大于",
    "INTEGRAL": "积分", "INTERSECTION": "交集", "UNION": "并集", "SUBSET": "子集", "SUPERSET": "超集",
    "GREEK": "希腊字母", "CAPITAL": "大写", "SMALL": "小写", "LETTER": "字母", "ROMAN": "罗马数字",
    "DIGIT": "数字", "NUMBER": "数字", "PARENTHESIZED": "括号", "FRACTION": "分数", "SUPERSCRIPT": "上标", "SUBSCRIPT": "下标",
    "ALPHA": "阿尔法", "BETA": "贝塔", "GAMMA": "伽马", "DELTA": "德尔塔", "EPSILON": "艾普西龙",
    "ZETA": "泽塔", "ETA": "伊塔", "THETA": "西塔", "IOTA": "约塔", "KAPPA": "卡帕",
    "LAMDA": "拉姆达 兰姆达", "LAMBDA": "拉姆达 兰姆达", "MU": "缪 微", "NU": "纽", "XI": "克西",
    "OMICRON": "奥米克戎", "PI": "派 圆周率", "RHO": "柔", "SIGMA": "西格玛", "TAU": "陶",
    "UPSILON": "宇普西龙", "PHI": "斐 黄金分割", "CHI": "希", "PSI": "普西", "OMEGA": "欧米伽 欧姆",
}


def fetch(url, fallback=None):
    for candidate in [url] + ([fallback] if fallback else []):
        try:
            with urllib.request.urlopen(candidate, timeout=15) as response:
                return response.read()
        except (urllib.error.URLError, TimeoutError):
            if candidate == (fallback or url):
                raise


def main():
    source = fetch(UCD_URL, UCD_MIRROR).decode("utf-8")
    annotations = {}
    for item in ET.fromstring(fetch(CLDR_URL)).iter("annotation"):
        key = item.attrib["cp"].replace("\ufe0f", "")
        record = annotations.setdefault(key, {"name": "", "keywords": []})
        if item.attrib.get("type") == "tts":
            record["name"] = item.text or ""
        elif item.text:
            record["keywords"].append(item.text)
    characters, currencies = {}, set()
    for line in source.splitlines():
        fields = line.split(";")
        cp, name, category = int(fields[0], 16), fields[1], fields[2]
        # Named, visible standalone characters only: omit controls, marks,
        # spaces, private-use, unassigned ranges, surrogates and formatting.
        if not name.startswith("<") and category[0] in "LNPS" and not chr(cp).isspace():
            characters[cp] = name
            if category == "Sc":
                currencies.add(cp)
    seen, rows = set(), []
    for group, zh, en, ranges, extra in GROUPS:
        points = set(map(ord, extra))
        if group == 5:
            points.update(currencies)
        for low, high in ranges:
            points.update(range(low, high + 1))
        for cp in sorted(points):
            if cp in seen or cp not in characters:
                continue
            seen.add(cp)
            char, name = chr(cp), characters[cp]
            annotation = annotations.get(char, {})
            translated_tokens = [TOKENS[token] for token in name.split() if token in TOKENS]
            translated_name = NAMES.get(char) or annotation.get("name")
            words = name.split()
            if not translated_name and group == 12 and len(words) == 4 and words[:1] == ["GREEK"] and words[2] == "LETTER" and words[3] in TOKENS:
                case = "大写" if words[1] == "CAPITAL" else "小写"
                translated_name = case + "希腊字母" + TOKENS[words[3]].split()[0]
            rows.append({
                "text": char,
                "name": translated_name or name.lower(),
                "name_en": name.lower(),
                "keywords": " ".join([zh, en, f"U+{cp:04X}", ALIASES.get(char, ""), *annotation.get("keywords", []), *translated_tokens]),
                "group": group,
            })
    rows.sort(key=lambda row: (row["group"], ord(row["text"])))
    assert len(rows) > 3000
    assert len({row["text"] for row in rows}) == len(rows)
    target = ROOT / "assets" / "emoji" / "symbols.json"
    target.write_text(json.dumps(rows, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"Generated {len(rows)} unique symbols; group counts: {dict(sorted(collections.Counter(row['group'] for row in rows).items()))}")


if __name__ == "__main__":
    main()
