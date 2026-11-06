# Bundled fonts

Every face the Preferences card offers is embedded in the binary (`FONT_FILES` in
`src/main.rs`), so a choice looks the same on a machine with none of them installed. All are
under the SIL Open Font License 1.1; each family's licence sits beside its files.

| Family | Files | Source |
| --- | --- | --- |
| Inter | Regular, SemiBold | https://github.com/rsms/inter |
| Geist | Regular, SemiBold | https://github.com/vercel/geist-font |
| IBM Plex Sans | Regular, SemiBold | https://github.com/IBM/plex |
| Newsreader (16pt optical size) | Regular, SemiBold, Italic | https://github.com/productiontype/Newsreader |
| Courier Prime | Regular, Bold, Italic | https://github.com/quoteunquoteapps/CourierPrime |

Only the weights the interface uses are included: regular and semibold, plus the italic a note
needs. Static TTFs rather than variable fonts, since gpui 0.2.2's loader is not known to
select weights from a variable face.
