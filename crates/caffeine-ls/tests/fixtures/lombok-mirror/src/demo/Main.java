package demo;

public class Main {
    public static void main(String[] args) {
        Person p = Person.of("alice");      // static factory: mirror only
        System.out.println(p.getName());    // getter: mirror only
        Person q = new Person("bob", 2);    // two-argument ctor: mirror only
        Person.PersonBuilder b = Person.builder(); // nested type + factory
        Person r = b.name("carol").build(); // builder methods: mirror only
        System.out.println(p.getAge());     // getter: mirror only
        System.out.println(r.mirrorField());
    }
}
