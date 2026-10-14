use rooste_testkit::OpenSshFixture;

#[test]
#[ignore = "requires the OpenSSH Docker fixture"]
fn openssh_fixture_speaks_ssh2() {
    let fixture = OpenSshFixture::from_env().expect("fixture address");
    let banner = fixture.read_banner().expect("read SSH banner");
    assert!(
        banner.starts_with("SSH-2.0-OpenSSH"),
        "unexpected banner: {banner}"
    );
}
