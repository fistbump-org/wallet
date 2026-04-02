// Minimal QR Code Generator — byte mode, EC level L, versions 1–20.
// Usage: QR.render(canvas, text, cellSize?)
var QR = (function() {
  'use strict';

  // ── GF(256) arithmetic ──
  var EXP = new Uint8Array(512), LOG = new Uint8Array(256);
  (function() {
    var x = 1;
    for (var i = 0; i < 255; i++) {
      EXP[i] = x; LOG[x] = i;
      x = (x << 1) ^ (x & 128 ? 0x11d : 0);
    }
    for (var i = 255; i < 512; i++) EXP[i] = EXP[i - 255];
  })();
  function gfMul(a, b) { return a && b ? EXP[LOG[a] + LOG[b]] : 0; }

  // Reed-Solomon generator polynomial
  function rsGenPoly(n) {
    var p = [1];
    for (var i = 0; i < n; i++) {
      var q = new Array(p.length + 1);
      for (var j = 0; j < q.length; j++) q[j] = 0;
      for (var j = 0; j < p.length; j++) {
        q[j] ^= p[j];
        q[j + 1] ^= gfMul(p[j], EXP[i]);
      }
      p = q;
    }
    return p;
  }

  // Reed-Solomon remainder
  function rsEncode(data, ecLen) {
    var gen = rsGenPoly(ecLen);
    var rem = new Array(ecLen);
    for (var i = 0; i < ecLen; i++) rem[i] = 0;
    for (var i = 0; i < data.length; i++) {
      var factor = data[i] ^ rem[0];
      rem.shift(); rem.push(0);
      for (var j = 0; j < ecLen; j++) rem[j] ^= gfMul(gen[j + 1], factor);
    }
    return rem;
  }

  // ── Version parameters (EC level L) ──
  // [totalCodewords, ecPerBlock, numBlocksGroup1, dataPerBlock1, numBlocksGroup2, dataPerBlock2]
  var VERSIONS = [
    null, // 0
    [26,7,1,19,0,0],[44,10,1,34,0,0],[70,15,1,55,0,0],[100,20,1,80,0,0],
    [134,26,1,108,0,0],[172,18,2,68,0,0],[196,20,2,78,0,0],[242,24,2,97,0,0],
    [292,30,2,116,0,0],[346,18,2,68,2,69],[404,20,4,81,0,0],[466,24,2,92,2,93],
    [532,26,4,107,0,0],[581,30,3,115,1,116],[655,22,5,87,1,88],[733,24,5,98,1,99],
    [815,28,1,107,5,108],[901,30,5,120,1,121],[991,28,3,113,4,114],[1085,28,3,107,5,108],
  ];

  // Alignment pattern center positions per version
  var ALIGN = [
    null,[],[6,18], [6,22],[6,26],[6,30],[6,34],[6,22,38],[6,24,42],[6,26,46],[6,28,50],
    [6,30,54],[6,32,58],[6,34,62],[6,26,46,66],[6,26,48,70],[6,26,50,74],
    [6,30,54,78],[6,30,56,82],[6,30,58,86],[6,34,62,90],
  ];

  function bestVersion(byteLen) {
    for (var v = 1; v <= 20; v++) {
      var p = VERSIONS[v];
      var dataCap = p[2] * p[3] + p[4] * p[5];
      // Byte mode overhead: 4 (mode) + charCountBits + data
      var ccBits = v <= 9 ? 8 : 16;
      var availBits = dataCap * 8;
      var needed = 4 + ccBits + byteLen * 8;
      if (needed <= availBits) return v;
    }
    return -1;
  }

  function encodeData(bytes, version) {
    var p = VERSIONS[version];
    var dataCap = p[2] * p[3] + p[4] * p[5];
    var ccBits = version <= 9 ? 8 : 16;

    // Build bit stream: mode(4) + count(ccBits) + data + terminator + padding
    var bits = [];
    function push(val, len) { for (var i = len - 1; i >= 0; i--) bits.push((val >> i) & 1); }

    push(4, 4); // byte mode indicator
    push(bytes.length, ccBits);
    for (var i = 0; i < bytes.length; i++) push(bytes[i], 8);
    push(0, Math.min(4, dataCap * 8 - bits.length)); // terminator

    // Pad to byte boundary
    while (bits.length % 8) bits.push(0);
    // Pad bytes
    var padBytes = [0xEC, 0x11], pi = 0;
    while (bits.length < dataCap * 8) {
      push(padBytes[pi], 8); pi ^= 1;
    }

    // Convert to bytes
    var data = new Uint8Array(dataCap);
    for (var i = 0; i < dataCap; i++) {
      var b = 0;
      for (var j = 0; j < 8; j++) b = (b << 1) | bits[i * 8 + j];
      data[i] = b;
    }
    return data;
  }

  function ecBlocks(data, version) {
    var p = VERSIONS[version];
    var ecLen = p[1];
    var blocks = [], ecAll = [];
    var offset = 0;
    for (var g = 0; g < 2; g++) {
      var count = g === 0 ? p[2] : p[4];
      var size = g === 0 ? p[3] : p[5];
      for (var i = 0; i < count; i++) {
        var block = Array.from(data.slice(offset, offset + size));
        blocks.push(block);
        ecAll.push(rsEncode(block, ecLen));
        offset += size;
      }
    }
    // Interleave data blocks
    var result = [];
    var maxData = Math.max.apply(null, blocks.map(function(b) { return b.length; }));
    for (var i = 0; i < maxData; i++)
      for (var j = 0; j < blocks.length; j++)
        if (i < blocks[j].length) result.push(blocks[j][i]);
    // Interleave EC blocks
    for (var i = 0; i < ecLen; i++)
      for (var j = 0; j < ecAll.length; j++)
        result.push(ecAll[j][i]);
    return result;
  }

  // ── Matrix construction ──
  function createMatrix(version) {
    var size = version * 4 + 17;
    var m = [], r = [];
    for (var i = 0; i < size; i++) {
      m[i] = new Uint8Array(size);
      r[i] = new Uint8Array(size); // reserved flag
    }
    return { m: m, r: r, size: size };
  }

  function setModule(mat, row, col, val) {
    mat.m[row][col] = val ? 1 : 0;
    mat.r[row][col] = 1;
  }

  function placeFinderPattern(mat, row, col) {
    for (var dr = -1; dr <= 7; dr++)
      for (var dc = -1; dc <= 7; dc++) {
        var r = row + dr, c = col + dc;
        if (r < 0 || r >= mat.size || c < 0 || c >= mat.size) continue;
        var inOuter = dr === -1 || dr === 7 || dc === -1 || dc === 7;
        var inRing = dr === 0 || dr === 6 || dc === 0 || dc === 6;
        var inCore = dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4;
        setModule(mat, r, c, !inOuter && (inRing || inCore) ? 1 : 0);
      }
  }

  function placeAlignmentPattern(mat, row, col) {
    for (var dr = -2; dr <= 2; dr++)
      for (var dc = -2; dc <= 2; dc++) {
        var v = Math.abs(dr) === 2 || Math.abs(dc) === 2 || (dr === 0 && dc === 0);
        setModule(mat, row + dr, col + dc, v ? 1 : 0);
      }
  }

  function placeFunctionPatterns(mat, version) {
    // Finder patterns
    placeFinderPattern(mat, 0, 0);
    placeFinderPattern(mat, 0, mat.size - 7);
    placeFinderPattern(mat, mat.size - 7, 0);

    // Alignment patterns (must be placed before timing so the overlap
    // check only catches finder patterns, not timing cells)
    if (version >= 2) {
      var pos = ALIGN[version];
      for (var i = 0; i < pos.length; i++)
        for (var j = 0; j < pos.length; j++) {
          if (mat.r[pos[i]][pos[j]]) continue; // skip if overlaps finder
          placeAlignmentPattern(mat, pos[i], pos[j]);
        }
    }

    // Timing patterns
    for (var i = 8; i < mat.size - 8; i++) {
      if (!mat.r[6][i]) setModule(mat, 6, i, i % 2 === 0 ? 1 : 0);
      if (!mat.r[i][6]) setModule(mat, i, 6, i % 2 === 0 ? 1 : 0);
    }

    // Dark module
    setModule(mat, mat.size - 8, 8, 1);

    // Reserve format info areas
    for (var i = 0; i < 8; i++) {
      if (!mat.r[8][i]) { mat.r[8][i] = 1; }
      if (!mat.r[8][mat.size - 1 - i]) { mat.r[8][mat.size - 1 - i] = 1; }
      if (!mat.r[i][8]) { mat.r[i][8] = 1; }
      if (!mat.r[mat.size - 1 - i][8]) { mat.r[mat.size - 1 - i][8] = 1; }
    }
    if (!mat.r[8][8]) mat.r[8][8] = 1;

    // Reserve version info areas (version >= 7)
    if (version >= 7) {
      for (var i = 0; i < 6; i++)
        for (var j = 0; j < 3; j++) {
          mat.r[i][mat.size - 11 + j] = 1;
          mat.r[mat.size - 11 + j][i] = 1;
        }
    }
  }

  function placeData(mat, codewords) {
    var size = mat.size;
    var bitIdx = 0;
    var totalBits = codewords.length * 8;
    // Traverse right-to-left in 2-column bands, bottom-to-top then top-to-bottom
    var col = size - 1;
    while (col >= 0) {
      if (col === 6) col--; // skip timing column
      var upward = ((size - 1 - col) >> 1) % 2 === 0;
      for (var cnt = 0; cnt < size; cnt++) {
        var row = upward ? size - 1 - cnt : cnt;
        for (var dx = 0; dx <= 1; dx++) {
          var c = col - dx;
          if (c < 0) continue;
          if (mat.r[row][c]) continue;
          if (bitIdx < totalBits) {
            var byteIdx = bitIdx >> 3;
            var bitPos = 7 - (bitIdx & 7);
            mat.m[row][c] = (codewords[byteIdx] >> bitPos) & 1;
          }
          bitIdx++;
        }
      }
      col -= 2;
    }
  }

  // ── Masking ──
  var MASKS = [
    function(r,c){return(r+c)%2===0},
    function(r,c){return r%2===0},
    function(r,c){return c%3===0},
    function(r,c){return(r+c)%3===0},
    function(r,c){return(Math.floor(r/2)+Math.floor(c/3))%2===0},
    function(r,c){return(r*c)%2+(r*c)%3===0},
    function(r,c){return((r*c)%2+(r*c)%3)%2===0},
    function(r,c){return((r+c)%2+(r*c)%3)%2===0},
  ];

  function applyMask(mat, maskIdx) {
    var fn = MASKS[maskIdx];
    for (var r = 0; r < mat.size; r++)
      for (var c = 0; c < mat.size; c++)
        if (!mat.r[r][c]) mat.m[r][c] ^= fn(r, c) ? 1 : 0;
  }

  function penalty(mat) {
    var s = mat.size, score = 0;
    // Rule 1: runs of 5+ same color
    for (var r = 0; r < s; r++) {
      var run = 1;
      for (var c = 1; c < s; c++) {
        if (mat.m[r][c] === mat.m[r][c-1]) run++;
        else { if (run >= 5) score += run - 2; run = 1; }
      }
      if (run >= 5) score += run - 2;
    }
    for (var c = 0; c < s; c++) {
      var run = 1;
      for (var r = 1; r < s; r++) {
        if (mat.m[r][c] === mat.m[r-1][c]) run++;
        else { if (run >= 5) score += run - 2; run = 1; }
      }
      if (run >= 5) score += run - 2;
    }
    // Rule 2: 2×2 blocks
    for (var r = 0; r < s - 1; r++)
      for (var c = 0; c < s - 1; c++) {
        var v = mat.m[r][c];
        if (v === mat.m[r][c+1] && v === mat.m[r+1][c] && v === mat.m[r+1][c+1]) score += 3;
      }
    // Rule 3: finder-like patterns (simplified)
    // Rule 4: proportion
    var dark = 0;
    for (var r = 0; r < s; r++) for (var c = 0; c < s; c++) if (mat.m[r][c]) dark++;
    var pct = dark * 100 / (s * s);
    score += Math.abs(Math.floor(pct / 5) * 5 - 50) * 2;
    return score;
  }

  // Format info bits for EC level L (= 01) with mask patterns 0–7
  var FORMAT_BITS = [0x77c4,0x72f3,0x7daa,0x789d,0x662f,0x6318,0x6c41,0x6976];
  // Version info bits (versions 7–20)
  var VERSION_BITS = [null,null,null,null,null,null,null,
    0x07C94,0x085BC,0x09A99,0x0A4D3,0x0BBF6,0x0C762,0x0D847,0x0E60D,
    0x0F928,0x10B78,0x1145D,0x12A17,0x13532,0x149A6];

  function placeFormatInfo(mat, maskIdx) {
    var bits = FORMAT_BITS[maskIdx];
    var s = mat.size;
    for (var i = 0; i < 15; i++) {
      var b = (bits >> (14 - i)) & 1;
      // Around top-left finder
      if (i < 6) setModule(mat, 8, i, b);
      else if (i === 6) setModule(mat, 8, 7, b);
      else if (i === 7) setModule(mat, 8, 8, b);
      else if (i === 8) setModule(mat, 7, 8, b);
      else setModule(mat, 14 - i, 8, b);
      // Second copy
      if (i < 7) setModule(mat, s - 1 - i, 8, b);
      else setModule(mat, 8, s - 15 + i, b);
    }
  }

  function placeVersionInfo(mat, version) {
    if (version < 7) return;
    var bits = VERSION_BITS[version];
    var s = mat.size;
    for (var i = 0; i < 18; i++) {
      var b = (bits >> i) & 1;
      var r = Math.floor(i / 3), c = s - 11 + (i % 3);
      mat.m[r][c] = b; mat.m[c][r] = b;
    }
  }

  function cloneMatrix(mat) {
    var m2 = [], r2 = [];
    for (var i = 0; i < mat.size; i++) {
      m2[i] = new Uint8Array(mat.m[i]);
      r2[i] = new Uint8Array(mat.r[i]);
    }
    return { m: m2, r: r2, size: mat.size };
  }

  // ── Public API ──

  function encode(text) {
    var bytes = [];
    for (var i = 0; i < text.length; i++) {
      var code = text.charCodeAt(i);
      if (code < 128) bytes.push(code);
      else if (code < 2048) { bytes.push(0xc0 | (code >> 6)); bytes.push(0x80 | (code & 0x3f)); }
      else { bytes.push(0xe0|(code>>12)); bytes.push(0x80|((code>>6)&0x3f)); bytes.push(0x80|(code&0x3f)); }
    }

    var version = bestVersion(bytes.length);
    if (version < 0) return null;

    var data = encodeData(bytes, version);
    var codewords = ecBlocks(data, version);
    var mat = createMatrix(version);
    placeFunctionPatterns(mat, version);
    placeData(mat, codewords);
    placeVersionInfo(mat, version);

    // Try all masks, pick lowest penalty
    var best = null, bestScore = Infinity;
    for (var mi = 0; mi < 8; mi++) {
      var trial = cloneMatrix(mat);
      applyMask(trial, mi);
      placeFormatInfo(trial, mi);
      var sc = penalty(trial);
      if (sc < bestScore) { bestScore = sc; best = trial; }
    }
    return best;
  }

  function render(canvas, text, cellSize) {
    cellSize = cellSize || 4;
    var mat = encode(text);
    if (!mat) return false;
    var quiet = 4; // quiet zone
    var total = mat.size + quiet * 2;
    canvas.width = total * cellSize;
    canvas.height = total * cellSize;
    var ctx = canvas.getContext('2d');
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.fillStyle = '#000000';
    for (var r = 0; r < mat.size; r++)
      for (var c = 0; c < mat.size; c++)
        if (mat.m[r][c])
          ctx.fillRect((c + quiet) * cellSize, (r + quiet) * cellSize, cellSize, cellSize);
    return true;
  }

  return { encode: encode, render: render };
})();
