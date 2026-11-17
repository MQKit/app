using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.Text;
using System.Web.Script.Serialization;

namespace OfficeWorker
{
    /// <summary>Thin wrapper around JavaScriptSerializer plus small helpers for reading untyped JSON.</summary>
    internal static class Json
    {
        private static JavaScriptSerializer Create()
        {
            return new JavaScriptSerializer { MaxJsonLength = int.MaxValue, RecursionLimit = 256 };
        }

        public static string Serialize(object value)
        {
            return Create().Serialize(value);
        }

        /// <summary>Parses a JSON object. Returns null when the text is not a JSON object. Throws on malformed JSON.</summary>
        public static Dictionary<string, object> ParseObject(string text)
        {
            return Create().DeserializeObject(text) as Dictionary<string, object>;
        }

        /// <summary>Builds an insertion-ordered object from key/value pairs.</summary>
        public static Dictionary<string, object> Obj(params object[] kv)
        {
            var d = new Dictionary<string, object>(StringComparer.Ordinal);
            for (int i = 0; i + 1 < kv.Length; i += 2)
                d[(string)kv[i]] = kv[i + 1];
            return d;
        }

        public static object[] Arr(params object[] items)
        {
            return items;
        }

        /// <summary>Bilingual text object {"ro":..., "en":...}.</summary>
        public static Dictionary<string, object> T(string ro, string en)
        {
            return Obj("ro", ro, "en", en);
        }

        public static object Get(Dictionary<string, object> d, string key)
        {
            if (d == null) return null;
            return d.TryGetValue(key, out var v) ? v : null;
        }

        public static string Str(Dictionary<string, object> d, string key)
        {
            return Get(d, key) as string;
        }

        public static Dictionary<string, object> O(Dictionary<string, object> d, string key)
        {
            return Get(d, key) as Dictionary<string, object>;
        }

        public static List<object> A(Dictionary<string, object> d, string key)
        {
            var v = Get(d, key);
            if (v is IList list && !(v is string))
            {
                var r = new List<object>(list.Count);
                foreach (var x in list) r.Add(x);
                return r;
            }
            return null;
        }

        public static bool? Bool(Dictionary<string, object> d, string key)
        {
            var v = Get(d, key);
            if (v is bool b) return b;
            return null;
        }

        public static long? Long(Dictionary<string, object> d, string key)
        {
            var v = Get(d, key);
            switch (v)
            {
                case int i: return i;
                case long l: return l;
                case decimal m: return (long)m;
                case double f: return (long)f;
                case string s when long.TryParse(s, NumberStyles.Integer, CultureInfo.InvariantCulture, out var p): return p;
                default: return null;
            }
        }

        /// <summary>Re-indents compact JSON (used for plugin.json files so humans can read them).</summary>
        public static string Pretty(string json)
        {
            var sb = new StringBuilder(json.Length * 2);
            int indent = 0;
            bool inString = false, escape = false;
            for (int i = 0; i < json.Length; i++)
            {
                char c = json[i];
                if (inString)
                {
                    sb.Append(c);
                    if (escape) escape = false;
                    else if (c == '\\') escape = true;
                    else if (c == '"') inString = false;
                    continue;
                }
                switch (c)
                {
                    case '"':
                        inString = true;
                        sb.Append(c);
                        break;
                    case '{':
                    case '[':
                        {
                            char close = c == '{' ? '}' : ']';
                            int j = i + 1;
                            while (j < json.Length && char.IsWhiteSpace(json[j])) j++;
                            if (j < json.Length && json[j] == close)
                            {
                                sb.Append(c).Append(close);
                                i = j;
                                break;
                            }
                            sb.Append(c).Append('\n');
                            indent++;
                            sb.Append(' ', indent * 2);
                            break;
                        }
                    case '}':
                    case ']':
                        sb.Append('\n');
                        indent--;
                        sb.Append(' ', indent * 2);
                        sb.Append(c);
                        break;
                    case ',':
                        sb.Append(c).Append('\n');
                        sb.Append(' ', indent * 2);
                        break;
                    case ':':
                        sb.Append(": ");
                        break;
                    default:
                        if (!char.IsWhiteSpace(c)) sb.Append(c);
                        break;
                }
            }
            sb.Append('\n');
            return sb.ToString();
        }

        public static string IsoUtc(DateTime t)
        {
            return t.ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss.fff'Z'", CultureInfo.InvariantCulture);
        }
    }
}
