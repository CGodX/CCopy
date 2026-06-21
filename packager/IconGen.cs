using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;

public static class IconGen
{
    public static void Run(string outPath)
    {
        int[] sizes = { 16, 32, 48, 64, 128, 256 };
        byte[][] pngs = new byte[sizes.Length][];
        for (int i = 0; i < sizes.Length; i++)
            pngs[i] = BuildPng(sizes[i]);

        using (var fs = File.Create(outPath))
        using (var bw = new BinaryWriter(fs))
        {
            // ICONDIR
            bw.Write((ushort)0);
            bw.Write((ushort)1);
            bw.Write((ushort)sizes.Length);

            int offset = 6 + 16 * sizes.Length;
            for (int i = 0; i < sizes.Length; i++)
            {
                byte w = sizes[i] == 256 ? (byte)0 : (byte)sizes[i];
                bw.Write(w);            // width
                bw.Write(w);            // height
                bw.Write((byte)0);      // palette
                bw.Write((byte)0);      // reserved
                bw.Write((ushort)1);    // planes
                bw.Write((ushort)32);   // bpp
                bw.Write((uint)pngs[i].Length);
                bw.Write((uint)offset);
                offset += pngs[i].Length;
            }
            for (int i = 0; i < sizes.Length; i++)
                bw.Write(pngs[i]);
        }
    }

    static byte[] BuildPng(int px)
    {
        using (var bmp = new Bitmap(px, px, PixelFormat.Format32bppArgb))
        {
            Color bg = Color.FromArgb(77, 124, 255);
            Color mark = Color.FromArgb(255, 255, 255);
            double scale = px / 32.0;
            for (int y = 0; y < px; y++)
            {
                for (int x = 0; x < px; x++)
                {
                    int gx = (int)(x / scale);
                    int gy = (int)(y / scale);
                    bool inMark =
                        (gx >= 8 && gx <= 23 && gy >= 8 && gy <= 11) ||
                        (gx >= 8 && gx <= 11 && gy >= 8 && gy <= 23) ||
                        (gx >= 8 && gx <= 23 && gy >= 20 && gy <= 23);
                    bmp.SetPixel(x, y, inMark ? mark : bg);
                }
            }
            using (var ms = new MemoryStream())
            {
                bmp.Save(ms, ImageFormat.Png);
                return ms.ToArray();
            }
        }
    }
}
