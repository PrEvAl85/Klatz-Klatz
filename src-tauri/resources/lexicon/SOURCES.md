# Word lists

`*.txt` in this folder are the frequency-ordered word lists the automatic
layout switcher uses to decide whether the text being typed belongs to the
language of the active layout.

Format: UTF-8, one lowercase word per line, ordered by frequency — the line
number is the rank, and only the first 10 000 words of each language are
kept. They are bundled with the application and read from disk at startup
rather than compiled in, so adding a language is a single file.

## Source

Generated from [`hermitdave/FrequencyWords`](https://github.com/hermitdave/FrequencyWords),
`content/2018`, which is built from the
[OpenSubtitles 2018](http://opus.nlpl.eu/OpenSubtitles2018.php) corpus.

Licensing of that repository: **MIT** for the code, **CC BY-SA 4.0** for the
content. The word lists in this folder are derived from the content and are
therefore distributed under CC BY-SA 4.0 as well.

## Regenerating

```powershell
.\tools\fetch-lexicons.ps1
```

The script downloads the lists, lowercases the words, strips everything that is
not a letter, drops duplicates and writes the first 10 000 words of each
language. `-Top` changes how many words are kept.

Note that a corpus of film subtitles is not a dictionary: it favours spoken
language, and names, slang and technical terms are over-represented. The lists
are good enough to tell languages apart, not to spell-check.