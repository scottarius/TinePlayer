//! Files a library keeps beside a video, named after it.
//!
//! One convention covers all of them: `<video name>.<tags>.<extension>`, with
//! whatever sits between the two carrying the language and any extra marks -
//! `Film (2019).en.hi.srt`, `Film (2019).en.ad.mp3`. Subtitle files have been
//! found this way since the beginning, and separate soundtracks are found by
//! the same rule rather than by one of their own: the tools that write them
//! write both, and a second rule would be a second thing to keep true.
//!
//! Nothing here reads a file. It answers what is sitting next to a video and
//! what its name says it is, which is all a list needs to offer it.

use std::path::{Path, PathBuf};

/// A file found beside a video and named after it.
pub struct Found {
    pub path: PathBuf,
    /// The file's own name, without its folder. What a choice is written down
    /// as for a subtitle, so that a library mounted somewhere else still
    /// resolves it.
    pub name: String,
    /// Whatever sits between the video's name and the extension, which is
    /// where the convention leaves the language and any extra marks. Empty for
    /// a file named exactly after the video, which is a real case and not a
    /// failure to match.
    pub tag: String,
}

/// Files beside `video` whose names begin with the video's own and end in one
/// of `extensions`.
///
/// In whatever order the directory answered in: what a list should be sorted
/// by is the label it ends up showing, which is the caller's to decide.
pub fn files(video: &Path, extensions: &[&str]) -> Vec<Found> {
    let Some(directory) = video.parent() else {
        return Vec::new();
    };
    let Some(stem) = video.file_stem().map(|s| s.to_string_lossy().to_string()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_string_lossy().to_lowercase();
            if !extensions.contains(&extension.as_str()) {
                return None;
            }

            // Compared without the extension, so that an upper-case one
            // doesn't defeat the trimming.
            let without_extension = path.file_stem()?.to_string_lossy().to_string();
            let rest = without_extension.strip_prefix(&stem)?;
            // The convention separates the name from the tag with a dot, and
            // holding to that is what keeps one film's files out of another's
            // list. A folder holding `Film.mkv` and `Film 2.mkv` would
            // otherwise offer `Film 2.en.mp3` as a soundtrack for `Film` - the
            // wrong film's audio over the right film's picture, and the same
            // mistake in the subtitle list, where it has been possible all
            // along and merely reads as an oddly named row.
            //
            // Nothing at all after the name is the other case that counts: a
            // file named exactly after the video, which is a real one and
            // comes back with an empty tag rather than being skipped.
            if !(rest.is_empty() || rest.starts_with('.')) {
                return None;
            }
            let tag = rest.trim_matches('.').to_string();

            Some(Found {
                name: path.file_name()?.to_string_lossy().to_string(),
                path,
                tag,
            })
        })
        .collect()
}

/// A separate soundtrack sitting beside the film: a described version, a dub,
/// or a restored track, downloaded next to it and named after it.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFile {
    pub path: PathBuf,
    /// Whatever the convention left between the film's name and the
    /// extension, where a library named this file after the film. Empty where
    /// it is named *exactly* after the film, which says nothing about itself,
    /// and `None` where nothing named it after the film at all.
    ///
    /// The difference is not decoration: a file named after the film is read
    /// from its tag alone, because its name is the film's with an extension on
    /// the end and reading that would read the film's title.
    pub tag: Option<String>,
    /// The file's own name, which is what tells one of these from the next
    /// where the tag says nothing.
    pub name: String,
}

impl AudioFile {
    /// How the row reads - see [`crate::label`], which every track in the
    /// application is named by, inside a file or beside it.
    ///
    /// Built when it is shown rather than when the file is found: the answer
    /// carries the interface language, and that can change without a restart.
    ///
    /// The tag rather than the whole file name, where there is one. The name
    /// is the film's over again with two letters on the end, those letters are
    /// the whole of what tells one of these from another, and a name long
    /// enough to be cut off is no use across a room.
    /// The language this file states, which is the first component of the tag
    /// where a library named it after the film - the same convention a
    /// subtitle file states its language by. Empty where nothing named it
    /// after the film, or where the leading component is not a language code.
    pub fn language(&self) -> &str {
        match &self.tag {
            Some(tag) => crate::label::split_tag(tag.trim()).0,
            None => "",
        }
    }

    /// Whether this is narration rather than a soundtrack, read from the same
    /// words a track title is read for - so `Film.en.ad.mka` beside a video and
    /// a stream titled "Audio Description" inside one arrive at the same
    /// answer.
    ///
    /// The tag where there is one, and the file's own name otherwise: `AD.mp3`
    /// is named after nothing and says what it is in the only place it can.
    pub fn is_described(&self) -> bool {
        let said = self.tag.as_deref().unwrap_or(&self.name);
        matches!(
            crate::label::kind_of_audio_tag(said),
            Some(crate::label::Kind::Described)
        )
    }

    /// How the row reads, at a given naming. See [`Self::label`] for the
    /// interface's own wording, which is this at [`crate::label::Naming::Native`].
    pub fn named(&self, naming: crate::label::Naming) -> String {
        match &self.tag {
            Some(tag) => crate::label::named_after_the_film(
                tag,
                &self.name,
                crate::label::kind_of_audio_tag,
                naming,
            ),
            None => crate::label::named_after_nothing(&self.name, crate::label::kind_of_audio_tag),
        }
    }

    pub fn label(&self) -> String {
        match &self.tag {
            Some(tag) => crate::label::named_after_the_film(
                tag,
                &self.name,
                crate::label::kind_of_audio_tag,
                crate::label::Naming::Native,
            ),
            None => crate::label::named_after_nothing(&self.name, crate::label::kind_of_audio_tag),
        }
    }
}

/// Every separate soundtrack beside `video`, in the order a list should show
/// them: the ones named after the film first, then anything else in a folder
/// that holds only this film.
pub fn audio(video: &Path) -> Vec<AudioFile> {
    let mut found: Vec<AudioFile> = files(video, crate::browser::AUDIO_EXTENSIONS)
        .into_iter()
        .map(|file| AudioFile {
            tag: Some(file.tag),
            name: file.name,
            path: file.path,
        })
        .collect();
    found.sort_by_key(AudioFile::label);

    // Then whatever else is in there, where there is only one film for it to
    // belong to. A soundtrack is *downloaded*, from somewhere that never heard
    // of the film's folder, and arrives called `AD.mp3` or `audio.mp3` or the
    // name it had on the site - so holding out for the convention means the
    // commonest way one of these arrives is the one way it is not offered.
    //
    // Safe only because it is the only film there. The rule the convention
    // earns its keep for is telling one film's files from another's in a
    // shared folder; where there is nothing to tell apart, an audio file in
    // the folder can only be for the film in the folder.
    //
    // Subtitles get the same treatment, in `subtitles::external`. This comment
    // used to say they deliberately did not, on the grounds that a subtitle
    // arrives named where a soundtrack does not - but a subtitle downloaded
    // from a subtitle site arrives as `English.srt` or `2_eng.srt` just as
    // readily, which is the same problem wearing a different extension.
    let mut loose: Vec<AudioFile> = in_a_lone_film_folder(video, crate::browser::is_audio)
        .into_iter()
        .filter(|path| !found.iter().any(|file| file.path == *path))
        .map(|path| AudioFile {
            // Named after nothing, so there is no tag - and its own name is
            // both what it is shown as and what is read for what it says,
            // since `AD.mp3` is how one of these arrives as often as not.
            tag: None,
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            path,
        })
        .collect();
    loose.sort_by_key(AudioFile::label);
    found.extend(loose);
    found
}

/// Every file in the video's folder that `wanted` accepts, where that folder
/// holds no other video. Empty as soon as there is a second one, which is what
/// keeps a trailer or an extras file from turning the rule loose on a shared
/// folder.
///
/// Takes what to look for rather than assuming, because soundtracks and
/// subtitles both want this and the answer to "is there only one film here"
/// must be the same for both. Two copies of the counting is two chances for
/// them to disagree about what a folder holds.
pub fn in_a_lone_film_folder(video: &Path, wanted: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Some(directory) = video.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut films = 0;
    let mut found = Vec::new();
    for path in entries.filter_map(|entry| entry.ok()).map(|e| e.path()) {
        if crate::browser::is_video(&path) {
            films += 1;
            if films > 1 {
                return Vec::new();
            }
        } else if wanted(&path) {
            found.push(path);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A film with the things a library really does leave beside one: a dub, a
    /// described track, subtitles, artwork, and a second film whose name
    /// happens to start the same way.
    fn library(root: &Path) -> PathBuf {
        let _ = std::fs::remove_dir_all(root);
        std::fs::create_dir_all(root).unwrap();
        for name in [
            "Film (2019).mkv",
            "Film (2019).en.mp3",
            "Film (2019).ad.mp3",
            "Film (2019).de.ac3",
            "Film (2019).en.hi.srt",
            "Film (2019).jpg",
            "Film (2019) Behind the Scenes.mkv",
            "Film (2019) Behind the Scenes.en.mp3",
            "Something Else.en.mp3",
        ] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        root.join("Film (2019).mkv")
    }

    #[test]
    fn finds_the_soundtracks_named_after_the_film() {
        let root = std::env::temp_dir().join("tp-beside-audio");
        let video = library(&root);

        let found = audio(&video);
        let labels: Vec<String> = found.iter().map(AudioFile::label).collect();
        // The subtitle, the artwork and the other film are not soundtracks for
        // this one, however they are named. Named as every other track in the
        // application is - see `crate::label`.
        assert_eq!(labels, ["Deutsch", "English", "ad - Audio Description"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The trap in matching on a name prefix: a second video in the folder
    /// whose name begins with this one's, with a soundtrack of its own.
    /// Matching the film's *stem* and not its whole name would claim it.
    #[test]
    fn a_longer_name_is_a_different_film() {
        let root = std::env::temp_dir().join("tp-beside-other");
        let video = library(&root);

        let claimed: Vec<String> = audio(&video)
            .into_iter()
            .map(|file| file.path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            !claimed
                .iter()
                .any(|name| name.contains("Behind the Scenes")),
            "took another film's soundtrack: {claimed:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Both films are found from their own name, which is the other half of
    /// the same rule: the extras have a soundtrack and it is theirs.
    #[test]
    fn the_other_film_finds_its_own() {
        let root = std::env::temp_dir().join("tp-beside-extras");
        library(&root);
        let extras = root.join("Film (2019) Behind the Scenes.mkv");

        let found = audio(&extras);
        assert_eq!(found.len(), 1, "{:?}", found.len());
        assert_eq!(found[0].label(), "English");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file named exactly after the video has nothing to read, so it is
    /// shown as itself rather than as an empty row.
    #[test]
    fn an_untagged_file_keeps_its_name() {
        let root = std::env::temp_dir().join("tp-beside-untagged");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Film.mkv"), b"x").unwrap();
        std::fs::write(root.join("Film.mka"), b"x").unwrap();

        let found = audio(&root.join("Film.mkv"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label(), "Film.mka");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An upper-case extension is the same extension. Files off a disc and
    /// out of older tools routinely carry one.
    #[test]
    fn the_extension_is_read_case_insensitively() {
        let root = std::env::temp_dir().join("tp-beside-case");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Film.mkv"), b"x").unwrap();
        std::fs::write(root.join("Film.FR.MP3"), b"x").unwrap();

        let found = audio(&root.join("Film.mkv"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label(), "Français");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A folder holding one film and a soundtrack named nothing in
    /// particular, which is how a downloaded one actually arrives. The film is
    /// the only thing it could belong to, so it is offered.
    fn lone_film(root: &Path) -> PathBuf {
        let _ = std::fs::remove_dir_all(root);
        std::fs::create_dir_all(root).unwrap();
        for name in [
            "Toy Story (1995).mp4",
            "Toy Story (1995).described.mp3",
            "AD.mp3",
            "audio.mp3",
            "Toy Story (1995).en.srt",
            "poster.jpg",
        ] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        root.join("Toy Story (1995).mp4")
    }

    #[test]
    fn a_lone_film_takes_whatever_audio_is_in_the_folder() {
        let root = std::env::temp_dir().join("tp-beside-lone");
        let video = lone_film(&root);

        let labels: Vec<String> = audio(&video).iter().map(AudioFile::label).collect();
        // The one named to the convention first, read; then the loose ones as
        // themselves, since their names name no language - though a name that
        // says what it is is still read for it.
        assert_eq!(
            labels,
            [
                "described - Audio Description",
                "AD.mp3 - Audio Description",
                "audio.mp3"
            ]
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A second film in the folder takes the loose rule away again: with two
    /// of them, a file called `AD.mp3` names no owner, and guessing one would
    /// play the wrong film's audio. What is named to the convention still
    /// stands, because that says whose it is.
    #[test]
    fn a_second_film_in_the_folder_settles_it_by_name_alone() {
        let root = std::env::temp_dir().join("tp-beside-two-films");
        let video = lone_film(&root);
        std::fs::write(root.join("Toy Story (1995)-trailer.mp4"), b"x").unwrap();

        let labels: Vec<String> = audio(&video).iter().map(AudioFile::label).collect();
        assert_eq!(labels, ["described - Audio Description"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// What each kind of tag reads as. The language and the description are
    /// read together, because a described track is usually in a language too.
    ///
    /// The reading itself is `crate::label`'s and is tested there; this is the
    /// half that matters here, which is that a file goes through it at all.
    #[test]
    fn a_tag_is_read_where_it_says_something() {
        let file = |tag: &str| AudioFile {
            path: PathBuf::new(),
            tag: Some(tag.to_string()),
            name: "Film (2019).mp3".to_string(),
        };
        assert_eq!(file("en").label(), "English");
        assert_eq!(file("ad").label(), "ad - Audio Description");
        assert_eq!(file("en.ad").label(), "English - Audio Description");
        // Nothing the table knows, and no description: shown as it stands
        // rather than guessed at.
        assert_eq!(file("commentary").label(), "commentary - Commentary");
        // Nothing to read at all, so the file's own name stands.
        assert_eq!(file("").label(), "Film (2019).mp3");
    }
}
