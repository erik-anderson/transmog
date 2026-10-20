// This source is compiled to JavaScript only; the native oracle keeps upstream Marshal reads.
using System;
using System.Collections.Generic;

namespace BlockParser
{
    public static partial class PortablePrimitives
    {
        public static int SizeOf(Type type)
        {
            type = Underlying(type);
            if (type == typeof(byte) || type == typeof(sbyte)) return 1;
            if (type == typeof(short) || type == typeof(ushort) || type == typeof(char)) return 2;
            if (type == typeof(int) || type == typeof(uint) || type == typeof(float) || type == typeof(bool)) return 4;
            if (type == typeof(long) || type == typeof(ulong) || type == typeof(double)) return 8;
            if (type == typeof(Guid)) return 16;
            throw new NotSupportedException(type.FullName);
        }

        public static object Read(Type type, byte[] bytes)
        {
            var primitive = Underlying(type);
            object value;
            if (primitive == typeof(byte)) value = bytes[0];
            else if (primitive == typeof(sbyte)) value = unchecked((sbyte)bytes[0]);
            else if (primitive == typeof(ushort)) value = BitConverter.ToUInt16(bytes, 0);
            else if (primitive == typeof(short)) value = BitConverter.ToInt16(bytes, 0);
            else if (primitive == typeof(uint)) value = BitConverter.ToUInt32(bytes, 0);
            else if (primitive == typeof(int)) value = BitConverter.ToInt32(bytes, 0);
            else if (primitive == typeof(ulong)) value = BitConverter.ToUInt64(bytes, 0);
            else if (primitive == typeof(long)) value = BitConverter.ToInt64(bytes, 0);
            else if (primitive == typeof(float)) value = BitConverter.ToSingle(bytes, 0);
            else if (primitive == typeof(double)) value = BitConverter.ToDouble(bytes, 0);
            else if (primitive == typeof(bool)) value = BitConverter.ToUInt32(bytes, 0) != 0;
            else if (primitive == typeof(char)) value = (char)BitConverter.ToUInt16(bytes, 0);
            else if (primitive == typeof(Guid)) value = new Guid(bytes);
            else throw new NotSupportedException(type.FullName);
            return type.IsEnum ? Enum.Parse(type, value.ToString()) : value;
        }

        public static string Ascii(byte[] bytes)
        {
            var result = new char[bytes.Length];
            for (int index = 0; index < bytes.Length; index++) result[index] = bytes[index] < 128 ? (char)bytes[index] : '.';
            return new string(result);
        }
    }
}

// Presentation colors have no rendering authority in the host-owned output.
namespace System.Drawing
{
    public enum Color { PaleVioletRed, Coral, LightPink, Salmon }
}

namespace System.Collections.Generic
{
    // The pinned partial-stream decoder uses dictionary operations and Keys.Last().
    // Enumerating Keys must preserve native SortedDictionary ordering, not insertion order.
    public class SortedDictionary<TKey, TValue> : Dictionary<TKey, TValue>
    {
        public new ICollection<TKey> Keys
        {
            get
            {
                var result = new List<TKey>(base.Keys);
                result.Sort();
                return result;
            }
        }
    }
}
